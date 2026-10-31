//! Reproduce the deployed initialization/upgrade path in a disposable schema database.
//! Never point HAGENCY_SCHEMA_DATABASE_URL at a live deployment.
use hagency_agent_service::{appservice::Inbox, domain::DomainStore, store::Store, transport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("HAGENCY_SCHEMA_DATABASE_URL")?;
    let _store = Store::open(
        &url,
        "schema.example.invalid",
        "https://schema.example.invalid/auth/",
    )
    .await?;
    let _domain = DomainStore::open(&url, "schema.example.invalid", "_hagency_").await?;
    let _inbox = Inbox::open(&url, "schema-generation-only-credential-00000000".into()).await?;
    let _transport = transport::TransportStore::open(&url, transport::Limits::default()).await?;
    Ok(())
}
