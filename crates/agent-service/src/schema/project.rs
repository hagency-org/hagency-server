// @generated automatically by Diesel CLI.

pub mod hagency_project_v1 {
    diesel::table! {
        hagency_project_v1.attachments (id) {
            id -> Text,
            project_id -> Text,
            task_id -> Text,
            filename -> Text,
            content_type -> Text,
            data -> Nullable<Bytea>,
            created_by -> Text,
            created_at -> Int8,
            removed_at -> Nullable<Int8>,
            byte_size -> Int8,
            content_sha256 -> Text,
        }
    }

    diesel::table! {
        hagency_project_v1.boards (project_id) {
            project_id -> Text,
            change_seq -> Int8,
            next_number -> Int8,
            owner_project_id -> Text,
            name -> Text,
            creation_key -> Nullable<Text>,
            created_by -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_project_v1.changes (project_id, seq) {
            project_id -> Text,
            seq -> Int8,
            task_id -> Nullable<Text>,
            actor -> Text,
            kind -> Text,
            occurred_at -> Int8,
            details -> Jsonb,
        }
    }

    diesel::table! {
        #[sql_name = "hagency_project_v1.columns"]
        hagency_project_v1.columns_ (project_id, column_key) {
            project_id -> Text,
            column_key -> Text,
            title -> Text,
            position -> Int4,
            wip_limit -> Nullable<Int8>,
            is_done -> Bool,
            archived -> Bool,
        }
    }

    diesel::table! {
        hagency_project_v1.comments (id) {
            id -> Text,
            project_id -> Text,
            task_id -> Text,
            author -> Text,
            body -> Text,
            created_at -> Int8,
            seq -> Int8,
        }
    }

    diesel::table! {
        hagency_project_v1.migrations (version) {
            version -> Int4,
        }
    }

    diesel::table! {
        hagency_project_v1.navigation_defaults (project_id) {
            project_id -> Text,
            board_id -> Nullable<Text>,
            room_id -> Nullable<Text>,
            revision -> Int8,
        }
    }

    diesel::table! {
        hagency_project_v1.notifications (project_id, recipient, seq) {
            project_id -> Text,
            recipient -> Text,
            seq -> Int8,
            task_id -> Text,
            actor -> Text,
            kind -> Text,
            title -> Text,
            details -> Jsonb,
            occurred_at -> Int8,
            read_at -> Nullable<Int8>,
        }
    }

    diesel::table! {
        hagency_project_v1.receipts (project_id, actor, intent_key) {
            project_id -> Text,
            actor -> Text,
            intent_key -> Text,
            digest -> Text,
            result -> Jsonb,
        }
    }

    diesel::table! {
        hagency_project_v1.tasks (id) {
            id -> Text,
            project_id -> Text,
            number -> Int8,
            title -> Text,
            description -> Text,
            state_key -> Text,
            rank -> Int8,
            priority -> Text,
            assignee_mxid -> Nullable<Text>,
            labels -> Jsonb,
            due_date -> Nullable<Date>,
            version -> Int8,
            created_by -> Text,
            created_at -> Int8,
            updated_at -> Int8,
            archived_at -> Nullable<Int8>,
            parent_id -> Nullable<Text>,
            checklist -> Jsonb,
        }
    }

    diesel::joinable!(changes -> boards (project_id));
    diesel::joinable!(columns_ -> boards (project_id));
    diesel::joinable!(receipts -> boards (project_id));
    diesel::joinable!(tasks -> boards (project_id));

    diesel::allow_tables_to_appear_in_same_query!(
        attachments,
        boards,
        changes,
        columns_,
        comments,
        migrations,
        navigation_defaults,
        notifications,
        receipts,
        tasks,
    );
}
