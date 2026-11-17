//! Opt-in project folder of an Agent space. The space creator names it when the
//! space is adopted. Hosting computers look it up per Room, so that every coding
//! Agent they run in the space works in that one folder.
use super::*;
use diesel::QueryableByName;

/// Discovery capability: adoption accepts `projectFolder` and
/// `GET /hagency/api/v1/room-project-folder` is served.
pub const PROJECT_FOLDER_CAPABILITY: &str = "space-project-folder-v1";

/// The one rule for a project folder name. It applies to the exact string, with
/// no trimming: 1 to 64 characters, no control characters, none of `/`, `\`,
/// `:` or NUL, no leading `.` and no leading or trailing whitespace.
pub fn validate_project_folder(name: &str) -> Result<()> {
    if !(1..=64).contains(&name.chars().count())
        || name.starts_with('.')
        || name.starts_with(char::is_whitespace)
        || name.ends_with(char::is_whitespace)
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '\0'))
    {
        return Err(Error::Invalid("invalid_project_folder"));
    }
    Ok(())
}

/// The folder of the project that a Room belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFolder {
    #[diesel(sql_type=Text)]
    pub project_id: String,
    #[diesel(sql_type=Text)]
    pub folder: String,
}

#[derive(QueryableByName)]
struct StoredFolder {
    #[diesel(sql_type=Text)]
    folder: String,
}

// A personal Agent is in a project Room through its binding. A pooled Agent never
// holds a binding (fence_managed_binding refuses one): it is in its assignment's
// original Room and in the Rooms added for it, and it counts there only for the
// user whose computer runs that live assignment.
const ROOM_FOLDER: &str = "SELECT p.id AS project_id,f.folder FROM hagency_agent_v1.rooms r \
 JOIN hagency_agent_v1.projects p ON p.id=r.project_id \
 JOIN hagency_agent_v1.project_folders f ON f.project_id=p.id \
 WHERE r.room_id=$1 AND r.active AND p.active AND (\
 EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id \
 WHERE b.room_id=r.room_id AND b.scope_kind='project' AND b.state IN ('joining','active','suspended') \
 AND (a.owner_user_id=$2 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments s \
 WHERE s.agent_id=b.agent_id AND s.executor_owner_user_id=$2))) \
 OR EXISTS(SELECT 1 FROM hagency_agent_v1.shared_agent_rooms b \
 JOIN hagency_agent_v1.management_shared_assignments s ON s.id=b.assignment_id AND s.agent_id=b.agent_id \
 WHERE b.room_id=r.room_id AND b.state IN ('joining','active') \
 AND s.state='pending_native' AND s.executor_owner_user_id=$2) \
 OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_matrix m \
 JOIN hagency_agent_v1.management_shared_assignments s ON s.id=m.assignment_id AND s.agent_id=m.agent_id \
 WHERE m.room_id=r.room_id AND m.state<>'fenced' \
 AND s.state='pending_native' AND s.executor_owner_user_id=$2))";

impl DomainStore {
    /// Stores the folder of a project the caller is registering in the same
    /// transaction. The first folder stays: the same name again is accepted, a
    /// different name is a conflict and rolls the whole adoption back.
    pub(super) async fn keep_project_folder(
        db: &mut AsyncPgConnection,
        p: &Principal,
        project: &str,
        folder: &str,
        now: i64,
    ) -> Result<String> {
        let inserted = sql_query("INSERT INTO hagency_agent_v1.project_folders(project_id,folder,set_by_user_id,created_at_ms) VALUES($1,$2,$3,$4) ON CONFLICT(project_id) DO NOTHING RETURNING folder")
            .bind::<Text, _>(project)
            .bind::<Text, _>(folder)
            .bind::<Text, _>(&p.user_id)
            .bind::<BigInt, _>(now)
            .get_result::<StoredFolder>(db)
            .await
            .optional()?;
        if let Some(stored) = inserted {
            Self::audit(db, p, "project.folder", project, now).await?;
            return Ok(stored.folder);
        }
        let stored =
            sql_query("SELECT folder FROM hagency_agent_v1.project_folders WHERE project_id=$1")
                .bind::<Text, _>(project)
                .get_result::<StoredFolder>(db)
                .await?;
        if stored.folder != folder {
            return Err(Error::Conflict("project_folder_conflict"));
        }
        Ok(stored.folder)
    }

    /// The folder of a Room's project, for a caller who owns an Agent in the Room
    /// or whose computer runs one there. `None` stands for every other case alike:
    /// no such Room, no folder, an inactive project or Room, or no such Agent.
    pub async fn room_project_folder(
        &self,
        p: &Principal,
        room: &str,
        now: i64,
    ) -> Result<Option<ProjectFolder>> {
        matrix_id(room, '!').map_err(|_| Error::Invalid("invalid_room_id"))?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Ok(sql_query(ROOM_FOLDER)
                        .bind::<Text, _>(room)
                        .bind::<Text, _>(&p.user_id)
                        .get_result::<ProjectFolder>(db)
                        .await
                        .optional()?)
                })
                .await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::validate_project_folder;
    use crate::Error;

    #[test]
    fn project_folder_names_follow_the_one_rule() {
        let longest = "a".repeat(64);
        let longest_wide = "é".repeat(64);
        for valid in [
            "a",
            "Team Room",
            "my-app_2",
            "项目",
            "app.v2",
            "folder.",
            "~",
            longest.as_str(),
            longest_wide.as_str(),
        ] {
            assert!(validate_project_folder(valid).is_ok(), "{valid:?}");
        }
        let too_long = "a".repeat(65);
        for invalid in [
            "",
            too_long.as_str(),
            ".",
            "..",
            ".hidden",
            " leading",
            "trailing ",
            "\tleading",
            "trailing\n",
            "\u{3000}wide space",
            "a/b",
            "/",
            "a\\b",
            "c:d",
            "nul\0byte",
            "line\nbreak",
            "bell\u{7}",
            "delete\u{7f}",
            "c1\u{85}control",
        ] {
            assert!(
                matches!(
                    validate_project_folder(invalid),
                    Err(Error::Invalid("invalid_project_folder"))
                ),
                "{invalid:?}"
            );
        }
    }
}
