use clap::Parser;
use hagency_server::{MatrixServer, browser_auth::BrowserAuth, config::Config, frontend::Frontend};
use salvo::prelude::*;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(
    version,
    about = "Matrix, OIDC and Hagency administration in one Rust server"
)]
struct Args {
    #[arg(short, long, default_value = "config/dev/hagency.toml")]
    config: PathBuf,
    #[arg(long)]
    check_config: bool,
    #[arg(long)]
    list_config_files: bool,
    /// Rename a stopped existing deployment's /_pasion/ identity to /auth/ and exit.
    #[arg(long, conflicts_with_all = ["bootstrap_admin", "check_config", "list_config_files"])]
    migrate_auth_path: bool,
    #[arg(long, requires = "bootstrap_password_file")]
    bootstrap_admin: Option<String>,
    #[arg(long, requires = "bootstrap_admin")]
    bootstrap_password_file: Option<PathBuf>,
    /// Explicitly link the first Pasion administrator to an existing Matrix administrator.
    #[arg(long, requires = "bootstrap_admin")]
    link_existing_matrix_admin: bool,
}
fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()?
        .block_on(run())
}
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.list_config_files {
        println!(
            "{}",
            serde_json::to_string(&Config::config_files(&args.config)?)?
        );
        return Ok(());
    }
    let mut conf = Config::load(&args.config)?;
    anyhow::ensure!(
        conf.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth),
        "Agent service requires Pasion delegated Matrix authentication"
    );
    if args.check_config {
        println!("Configuration is valid.");
        return Ok(());
    }
    if args.migrate_auth_path {
        let issuer = conf.public_origin.join(hagency_server::pasion::MOUNT)?;
        let changed = hagency_agent_service::store::Store::migrate_auth_path(
            &conf.database_url,
            conf.matrix.server_name.as_str(),
            issuer.as_str(),
        )
        .await?;
        println!(
            "Auth path migration {}.",
            if changed {
                "completed; sign in again"
            } else {
                "already completed"
            }
        );
        return Ok(());
    }
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .with(palpo::logging::capture_layer())
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(opentelemetry::global::tracer("hagency-server")),
        )
        .init();
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Rustls crypto provider was already initialized"))?;
    conf.prepare_signing_key()?;
    let pasion_config = hagency_server::pasion::prepare(&mut conf).await?;
    let pasion = match &pasion_config {
        Some(figment) => {
            Some(hagency_server::PasionServer::initialize(figment, Default::default()).await?)
        }
        None => None,
    };
    let matrix = MatrixServer::initialize(conf.matrix.clone())
        .await
        .map_err(anyhow::Error::msg)?;
    if let Some(localpart) = &args.bootstrap_admin {
        if conf.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth) {
            hagency_server::pasion::bootstrap_admin(
                pasion_config.as_ref().unwrap(),
                conf.matrix.server_name.as_str(),
                localpart,
                args.bootstrap_password_file.as_ref().unwrap(),
                args.link_existing_matrix_admin,
            )
            .await?;
        } else {
            anyhow::ensure!(
                !args.link_existing_matrix_admin,
                "linking an existing administrator requires Pasion delegated authentication"
            );
            let user = palpo::core::OwnedUserId::try_from(format!(
                "@{localpart}:{}",
                conf.matrix.server_name
            ))?;
            anyhow::ensure!(user.validate_strict().is_ok(), "invalid bootstrap username");
            anyhow::ensure!(
                !palpo::data::user::user_exists(&user).await?,
                "bootstrap refuses to modify an existing user"
            );
            let password = std::fs::read_to_string(args.bootstrap_password_file.as_ref().unwrap())?;
            let password = password.trim_end_matches(['\r', '\n']);
            anyhow::ensure!(
                password.len() >= 12,
                "bootstrap password must be at least 12 characters"
            );
            palpo::user::create_user(&user, Some(password)).await?;
            palpo::user::make_user_admin(&user).await?;
            tracing::info!(user=%user,"bootstrap administrator created");
        }
    }
    // The new user/device domain uses its own schema and never imports Fleet state.
    let mut agent_service = if conf.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth) {
        let issuer = conf.public_origin.join("auth/")?;
        let agent_store = hagency_agent_service::store::Store::open(
            &conf.database_url,
            conf.matrix.server_name.as_str(),
            issuer.as_str(),
        )
        .await?;
        let verifier = hagency_agent_service::identity::Verifier::new(
            issuer.clone(),
            conf.internal_origin().join("auth/oauth2/introspect")?,
            conf.internal_origin(),
            conf.matrix
                .admin
                .mas_secret
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing Pasion introspection credential"))?,
            conf.matrix.server_name.to_string(),
        )?;
        Some(hagency_agent_service::api::App::new(
            agent_store,
            verifier,
            conf.public_origin.clone(),
            issuer,
            conf.matrix.server_name.to_string(),
        ))
    } else {
        None
    };
    let mut agent_inbox = if agent_service.is_some() {
        let registration = hagency_server::agent_appservice::prepare(&conf).await?;
        if let Some(app) = agent_service.take() {
            agent_service = Some(app.with_matrix_client(
                hagency_agent_service::matrix_client::MatrixClient::new(
                    conf.internal_origin(),
                    registration.as_token.clone(),
                    conf.matrix.server_name.to_string(),
                )?,
            ));
        }
        Some(
            hagency_agent_service::appservice::Inbox::open(
                &conf.database_url,
                registration.hs_token.clone(),
            )
            .await?
            .with_limits(hagency_agent_service::appservice::Limits {
                max_pending: conf.queue.max_pending as i64,
                max_records: conf.queue.max_records as i64,
                max_bytes: conf.queue.max_bytes as i64,
            })?,
        )
    } else {
        None
    };
    if let Some(app) = agent_service.take() {
        let domain = hagency_agent_service::domain::DomainStore::open(
            &conf.database_url,
            conf.matrix.server_name.as_str(),
            "_hagency_",
        )
        .await?;
        let read: hagency_agent_service::gateway::ReadState = Arc::new(|room| {
            Box::pin(async move {
                let room: palpo::core::OwnedRoomId = room
                    .try_into()
                    .map_err(|_| hagency_agent_service::Error::Invalid("invalid_room_id"))?;
                let frame = palpo::room::get_current_frame_id(&room)
                    .await
                    .map_err(|_| {
                        hagency_agent_service::Error::Unavailable("matrix_state_unavailable")
                    })?
                    .ok_or(hagency_agent_service::Error::Unavailable(
                        "matrix_state_unavailable",
                    ))?;
                let expected = palpo::room::state::get_full_state_ids(frame)
                    .await
                    .map_err(|_| {
                        hagency_agent_service::Error::Unavailable("matrix_state_unavailable")
                    })?
                    .values()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let state = palpo::room::state::get_full_state(frame)
                    .await
                    .map_err(|_| {
                        hagency_agent_service::Error::Unavailable("matrix_state_unavailable")
                    })?;
                let events = state
                    .values()
                    .map(|event| {
                        serde_json::to_value(event).map_err(|_| {
                            hagency_agent_service::Error::Unavailable("matrix_state_unavailable")
                        })
                    })
                    .collect::<hagency_agent_service::Result<Vec<_>>>()?;
                hagency_server::agent_appservice::validate_state_completeness(&expected, &events)?;
                Ok(events)
            })
        });
        let gateway = hagency_agent_service::gateway::Gateway::new(
            read,
            format!("@_hagency_service:{}", conf.matrix.server_name),
        );
        if let Some(inbox) = agent_inbox.take() {
            let known_domain = domain.clone();
            let service_mxid = format!("@_hagency_service:{}", conf.matrix.server_name);
            agent_inbox = Some(inbox.with_known_user(Arc::new(move |mxid| {
                let domain = known_domain.clone();
                let service = service_mxid.clone();
                Box::pin(async move {
                    if mxid == service {
                        Ok(true)
                    } else {
                        domain.known_puppet(&mxid).await
                    }
                })
            })));
        }
        let transport = hagency_agent_service::transport::TransportStore::open(
            &conf.database_url,
            hagency_agent_service::transport::Limits {
                event_ttl_ms: conf.queue.event_ttl_ms,
                ..Default::default()
            },
        )
        .await?;
        agent_service = Some(app.with_domain(domain, gateway).with_transport(transport));
    }
    let browser_auth = BrowserAuth::new(&conf).await?;
    let mut matrix_router = Router::new();
    if conf.pasion.as_ref().is_some_and(|p| p.delegate_matrix_auth) {
        matrix_router =
            matrix_router.hoop(hagency_server::pasion::DelegatedAdminGuard::new(&conf)?);
    }
    matrix_router = matrix_router.push(matrix.router());
    let mut router = Router::new();
    if let Some(service) = &agent_service {
        router = router.push(service.router());
    }
    if let Some(inbox) = &agent_inbox {
        router = router.push(inbox.router());
    }
    if let Some(app) = &agent_service {
        router = router.push(app.readiness_router());
    }
    let frontend = Frontend::new(&conf);
    frontend.preserve_assets().await?;
    let mut router = router
        .push(browser_auth.router())
        .push(Router::with_path("hagency/api/{**path}").goal(unknown_hagency_api))
        .push(Router::with_path("api/{**path}").goal(unknown_hagency_api))
        .push(matrix_router)
        .push(Router::with_path("healthz").get(health))
        .push(frontend.router());
    if let Some(pasion) = &pasion {
        router = Router::new()
            .push(pasion.router(
                &conf.pasion.as_ref().unwrap().resources()?,
                Some(hagency_server::pasion::MOUNT),
            ))
            .push(router);
    }
    let mut service = matrix.service(router);
    // Append would run inside embedded CORS; merge Vary after CORS completes.
    service.hoops.insert(
        0,
        std::sync::Arc::new(hagency_server::frontend::frontend_vary),
    );
    let identity_worker = agent_service
        .as_ref()
        .zip(agent_inbox.as_ref())
        .and_then(|(app, inbox)| app.start_matrix_readiness(inbox.clone()));
    let delivery_worker = agent_service
        .as_ref()
        .zip(agent_inbox.as_ref())
        .and_then(|(app, inbox)| app.start_delivery(inbox.clone()));
    let cleanup_worker = agent_service.as_ref().and_then(|app| app.start_cleanup());
    let acceptor = TcpListener::new(conf.listen).try_bind().await?;
    let server = Server::new(acceptor);
    let handle = server.handle();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {_=ctrl_c=>(),_=terminate.recv()=>()}
        }
        #[cfg(not(unix))]
        ctrl_c.await.expect("shutdown signal");
        handle.stop_graceful(Some(Duration::from_secs(15)));
    });
    tracing::info!(listen=%conf.listen,public_origin=%conf.public_origin,"hagency-server started");
    server.serve(service).await;
    if let Some(worker) = identity_worker {
        worker.abort();
        let _ = worker.await;
    }
    if let Some(worker) = delivery_worker {
        worker.abort();
        let _ = worker.await;
    }
    if let Some(worker) = cleanup_worker {
        worker.abort();
        let _ = worker.await;
    }
    if let Some(pasion) = pasion {
        tokio::time::timeout(Duration::from_secs(15), pasion.shutdown())
            .await
            .map_err(|_| anyhow::anyhow!("Pasion workers did not stop within 15 seconds"))?;
    }
    Ok(())
}
#[handler]
async fn health() -> &'static str {
    "ok"
}
#[handler]
async fn unknown_hagency_api(res: &mut Response) {
    res.status_code(StatusCode::NOT_FOUND);
    res.render(Json(serde_json::json!({"code":"not_found"})));
}
