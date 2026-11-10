//! Owner ledger epochs. A native owner ledger numbers its own mutations from 1,
//! so a device that replaces its ledger database would reuse numbers the old
//! database already reported. Each ledger a device declares opens its next
//! epoch, and receipt numbers collide only inside one epoch. A device that
//! never declares stays in epoch 0, which is every receipt recorded before
//! declarations existed.
use super::*;
use crate::management_agent_executor_types::{ExecutorLedger, ExecutorLedgerInput};

#[derive(QueryableByName)]
struct Declared {
    #[diesel(sql_type=Text)]
    ledger_id: String,
    #[diesel(sql_type=BigInt)]
    epoch: i64,
}

/// The epoch this device's new receipts are numbered in. Callers hold the
/// domain transaction lock taken by `authorize`, as a declaration does.
pub(in crate::domain::management) async fn current_epoch(
    db: &mut AsyncPgConnection,
    owner: &str,
    device: &str,
) -> Result<i64> {
    Ok(sql_query("SELECT coalesce(max(epoch),0)::bigint AS now FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2")
        .bind::<Text, _>(owner)
        .bind::<Text, _>(device)
        .get_result::<Clock>(db)
        .await?
        .now)
}

impl DomainStore {
    /// Declares the ledger database the calling executor device now reports
    /// from. Repeating the current id changes nothing. An id from an earlier
    /// epoch is never accepted again: its numbers were already used.
    pub async fn declare_executor_ledger(
        &self,
        p: &Principal,
        input: ExecutorLedgerInput,
    ) -> Result<ExecutorLedger> {
        input.validate()?;
        let (device, _) = device(p)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                // The domain transaction lock serializes concurrent declarations
                // and every receipt of this device. The table's unique keys
                // are the backstop.
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let current = sql_query("SELECT ledger_id,epoch FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2 ORDER BY epoch DESC LIMIT 1")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .get_result::<Declared>(db)
                    .await
                    .optional()?;
                if let Some(current) = &current
                    && current.ledger_id == input.ledger_id
                {
                    return Ok(ExecutorLedger {
                        ledger_id: current.ledger_id.clone(),
                        epoch: current.epoch,
                    });
                }
                let retired = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_ledgers WHERE owner_user_id=$1 AND executor_device_id=$2 AND ledger_id=$3) AS matched")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .bind::<Text, _>(&input.ledger_id)
                    .get_result::<Flag>(db)
                    .await?
                    .matched;
                if retired {
                    return Err(Error::Conflict("agent_executor_ledger_retired"));
                }
                let epoch = current.map_or(0, |current| current.epoch) + 1;
                sql_query("INSERT INTO hagency_agent_v1.management_executor_ledgers(owner_user_id,executor_device_id,ledger_id,epoch,declared_at_ms) VALUES($1,$2,$3,$4,$5)")
                    .bind::<Text, _>(&p.user_id)
                    .bind::<Text, _>(device)
                    .bind::<Text, _>(&input.ledger_id)
                    .bind::<BigInt, _>(epoch)
                    .bind::<BigInt, _>(now)
                    .execute(db)
                    .await?;
                Self::audit(db, p, "executor.ledger.declare", &input.ledger_id, now).await?;
                Ok(ExecutorLedger {
                    ledger_id: input.ledger_id.clone(),
                    epoch,
                })
            })
            .await
    }
}
