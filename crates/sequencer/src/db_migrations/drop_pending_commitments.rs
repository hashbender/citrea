use std::sync::Arc;

use sov_db::ledger_db::migrations::LedgerMigration;
use sov_db::ledger_db::LedgerDB;
use tracing::info;

/// Migration to drop fullnode tables that were removed from SEQUENCER_LEDGER_TABLES
pub struct DropPendingCommitments;

impl LedgerMigration for DropPendingCommitments {
    fn identifier(&self) -> (String, u64) {
        ("drop_pending_commitments".to_string(), 1)
    }

    fn execute(
        &self,
        _ledger_db: Arc<LedgerDB>,
        tables_to_drop: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        let fullnode_tables_to_drop = vec!["PendingSequencerCommitment"];

        for table in fullnode_tables_to_drop {
            tables_to_drop.push(table.to_string());
            info!("Removing table '{}'", table);
        }

        Ok(())
    }
}
