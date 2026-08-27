use crate::application::outcome::Effect;

/// What a delete/archive should do after the local removal.
pub enum Mutation {
    /// Plain IMAP delete (`\Deleted` + EXPUNGE).
    Delete,
    /// Move to another mailbox (Archive).
    MoveTo(String),
}

impl Mutation {
    /// The `dest` of the matching [`Effect::DeleteOnServer`] (`None` = delete).
    pub fn dest(&self) -> Option<String> {
        match self {
            Mutation::Delete => None,
            Mutation::MoveTo(f) => Some(f.clone()),
        }
    }
}

/// The plan for a bulk delete/archive over `uids` in `folder`.
pub struct DeletePlan {
    /// UIDs to remove from the local lists + cache right away.
    pub remove_now: Vec<u64>,
    /// When set, the server-side op to schedule after removing (online only).
    pub effect: Option<Effect>,
    /// True when the plan came from the online (optimistic) path, so the shell
    /// can word the toast by target count rather than success count.
    pub optimistic: bool,
}

impl DeletePlan {
    /// Online plan: optimistic removal of every target plus one batched
    /// background server op.
    pub fn online(folder: &str, mutation: &Mutation, uids: &[u64]) -> Self {
        Self {
            remove_now: uids.to_vec(),
            effect: Some(Effect::DeleteOnServer {
                folder: folder.to_string(),
                uids: uids.to_vec(),
                dest: mutation.dest(),
            }),
            optimistic: true,
        }
    }

    /// Offline plan: each uid is attempted through the shell's current source;
    /// only successes land in `remove_now`. No server round-trip.
    pub fn offline(succeeded: Vec<u64>) -> Self {
        Self {
            remove_now: succeeded,
            effect: None,
            optimistic: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_plan_removes_everything_and_schedules_one_batch() {
        let plan = DeletePlan::online("INBOX", &Mutation::MoveTo("Archive".into()), &[1, 2, 3]);
        assert_eq!(plan.remove_now, vec![1, 2, 3]);
        assert!(plan.optimistic);
        match plan.effect {
            Some(Effect::DeleteOnServer { folder, uids, dest }) => {
                assert_eq!(folder, "INBOX");
                assert_eq!(uids, vec![1, 2, 3]);
                assert_eq!(dest.as_deref(), Some("Archive"));
            }
            _ => panic!("expected a server effect"),
        }
    }

    #[test]
    fn delete_mutation_has_no_destination() {
        let plan = DeletePlan::online("INBOX", &Mutation::Delete, &[9]);
        match plan.effect {
            Some(Effect::DeleteOnServer { dest, .. }) => assert!(dest.is_none()),
            _ => panic!("expected a server effect"),
        }
    }

    #[test]
    fn offline_plan_only_reports_succeeded_uids() {
        // The shell attempts each uid through its source and reports successes;
        // the plan itself schedules nothing.
        let plan = DeletePlan::offline(vec![2, 4]);
        assert_eq!(plan.remove_now, vec![2, 4]);
        assert!(!plan.optimistic);
        assert!(plan.effect.is_none());
    }
}
