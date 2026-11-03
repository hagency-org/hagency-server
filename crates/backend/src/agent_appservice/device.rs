//! Upgrade only the built-in service's device capability, retaining its identity.
use diesel::prelude::*;
use diesel_async::{AsyncConnection, RunQueryDsl};
use palpo::core::appservice::Registration;
use palpo::data::appservice::DbRegistration;
use palpo::data::schema::appservice_registrations as table;

fn upgrade_needed(expected: &Registration, stored: &DbRegistration) -> anyhow::Result<bool> {
    anyhow::ensure!(
        expected.id == super::ID && expected.device_management,
        "Agent Appservice requires device management"
    );
    anyhow::ensure!(!stored.disabled, "Agent Appservice is disabled");
    let mut observed: Registration = stored.clone().try_into()?;
    let upgrade = !observed.device_management;
    observed.device_management = true;
    anyhow::ensure!(
        serde_json::to_value(observed)? == serde_json::to_value(expected)?,
        "Agent Appservice registration conflicts with persisted deployment identity"
    );
    Ok(upgrade)
}

pub(super) async fn prepare(expected: &Registration) -> anyhow::Result<()> {
    let candidate = DbRegistration::from(expected.clone());
    let mut conn = palpo::data::connect().await?;
    conn.transaction::<_, anyhow::Error, _>(async |conn| {
        // An insert race must converge on the same identity, not replace tokens.
        diesel::insert_into(table::table)
            .values(&candidate)
            .on_conflict(table::id)
            .do_nothing()
            .execute(conn)
            .await?;
        let stored = table::table
            .find(super::ID)
            .for_update()
            .first::<DbRegistration>(conn)
            .await?;
        if upgrade_needed(expected, &stored)? {
            diesel::update(table::table.find(super::ID))
                .set(table::device_management.eq(true))
                .execute(conn)
                .await?;
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expected() -> Registration {
        serde_json::from_value(json!({
            "id":super::super::ID, "url":"http://127.0.0.1:8088",
            "as_token":"fixture-as-token", "hs_token":"fixture-hs-token",
            "sender_localpart":"_hagency_service", "io.element.msc4190":true,
            "namespaces":{"users":[{"exclusive":true,"regex":"^@_hagency_[a-z0-9_]+:example\\.test$"}],"aliases":[],"rooms":[]}
        })).unwrap()
    }

    #[test]
    fn device_upgrade_accepts_only_the_exact_enabled_builtin_identity() {
        let expected = expected();
        let mut legacy = DbRegistration::from(expected.clone());
        assert!(!upgrade_needed(&expected, &legacy).unwrap());
        legacy.device_management = false;
        assert!(upgrade_needed(&expected, &legacy).unwrap());
        let original = legacy.clone();
        legacy.disabled = true;
        assert!(upgrade_needed(&expected, &legacy).is_err());
        for field in [
            "as_token",
            "hs_token",
            "url",
            "sender_localpart",
            "namespaces",
        ] {
            let mut value = serde_json::to_value(&original).unwrap();
            value[field] = if field == "namespaces" {
                json!({"users":[],"aliases":[],"rooms":[]})
            } else {
                json!("changed")
            };
            let changed = serde_json::from_value(value).unwrap();
            assert!(upgrade_needed(&expected, &changed).is_err(), "{field}");
        }
    }

    #[tokio::test]
    #[ignore = "requires an empty dedicated HAGENCY_APPSERVICE_TEST_DATABASE_URL"]
    async fn database_device_capability_upgrade_preserves_identity_and_disabled_state() {
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            count: i64,
        }
        let url = std::env::var("HAGENCY_APPSERVICE_TEST_DATABASE_URL").unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        assert!(matches!(parsed.host_str(), Some("localhost" | "127.0.0.1")));
        assert!(parsed.path().starts_with("/crypto_"));
        let mut connection = diesel::PgConnection::establish(&url).unwrap();
        let count: Count = diesel::RunQueryDsl::get_result(
            diesel::sql_query("SELECT count(*) AS count FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema')"),
            &mut connection,
        ).unwrap();
        assert_eq!(count.count, 0, "requires an empty disposable database");
        drop(connection);
        let config =
            serde_json::from_value(json!({"url":url,"pool_size":4,"statement_timeout":5000}))
                .unwrap();
        palpo::data::init(&config).unwrap();
        let expected = expected();
        prepare(&expected).await.unwrap();
        let read = async || {
            palpo::data::appservice::find_registration(super::super::ID)
                .await
                .unwrap()
                .unwrap()
        };
        let initial = serde_json::to_value(read().await).unwrap();
        let mut conn = palpo::data::connect().await.unwrap();
        diesel::update(table::table.find(super::super::ID))
            .set(table::device_management.eq(false))
            .execute(&mut conn)
            .await
            .unwrap();
        let (one, two) = tokio::join!(prepare(&expected), prepare(&expected));
        one.unwrap();
        two.unwrap();
        assert_eq!(serde_json::to_value(read().await).unwrap(), initial);

        diesel::update(table::table.find(super::super::ID))
            .set((table::disabled.eq(true), table::device_management.eq(false)))
            .execute(&mut conn)
            .await
            .unwrap();
        let disabled = serde_json::to_value(read().await).unwrap();
        assert!(prepare(&expected).await.is_err());
        assert_eq!(serde_json::to_value(read().await).unwrap(), disabled);

        diesel::update(table::table.find(super::super::ID))
            .set((
                table::disabled.eq(false),
                table::as_token.eq("different-fixture-token"),
            ))
            .execute(&mut conn)
            .await
            .unwrap();
        let different = serde_json::to_value(read().await).unwrap();
        let error = prepare(&expected).await.unwrap_err().to_string();
        assert!(!error.contains("fixture-token"));
        assert_eq!(serde_json::to_value(read().await).unwrap(), different);
    }
}
