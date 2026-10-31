// @generated automatically by Diesel CLI.

pub mod hagency_agent_v1 {
    diesel::table! {
        hagency_agent_v1.agent_crypto_identities (agent_id, execution_device_id) {
            agent_id -> Text,
            execution_device_id -> Text,
            first_agent_generation -> Int8,
            first_device_generation -> Int8,
            matrix_device_id -> Text,
            device_keys_digest -> Nullable<Text>,
            master_key_digest -> Nullable<Text>,
            self_signing_key_digest -> Nullable<Text>,
            user_signing_key_digest -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.agent_threads (binding_id, thread_root) {
            binding_id -> Text,
            thread_root -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.agents (id) {
            id -> Text,
            owner_user_id -> Text,
            puppet_mxid -> Text,
            execution_device_id -> Nullable<Text>,
            display_name -> Text,
            state -> Text,
            generation -> Int8,
            owner_direct_room_id -> Nullable<Text>,
            visibility -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.bindings (id) {
            id -> Text,
            agent_id -> Text,
            project_id -> Nullable<Text>,
            room_id -> Text,
            scope_kind -> Text,
            state -> Text,
            generation -> Int8,
            admin_project_paused -> Bool,
            admin_room_paused -> Bool,
            owner_service_paused -> Bool,
            thread_auto_reply -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.ciphertext_inbox (id) {
            id -> Text,
            transaction_id -> Text,
            binding_id -> Text,
            agent_id -> Text,
            owner_user_id -> Text,
            agent_generation -> Int8,
            binding_generation -> Int8,
            event_id -> Text,
            room_id -> Text,
            sender_mxid -> Text,
            source_digest -> Text,
            event -> Nullable<Jsonb>,
            received_at_ms -> Int8,
            state -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.configuration_observations (agent_id) {
            agent_id -> Text,
            agent_generation -> Int8,
            execution_device_id -> Text,
            device_generation -> Int8,
            observed_at_ms -> Int8,
            environment -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.configuration_presence (device_id) {
            device_id -> Text,
            device_generation -> Int8,
            last_seen_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.configuration_requests (id) {
            id -> Text,
            owner_user_id -> Text,
            agent_id -> Text,
            agent_generation -> Int8,
            execution_device_id -> Text,
            device_generation -> Int8,
            requester_device_id -> Text,
            requester_generation -> Int8,
            idempotency_key -> Text,
            digest -> Text,
            operation -> Text,
            input -> Text,
            mutation -> Bool,
            state -> Text,
            created_at_ms -> Int8,
            expires_at_ms -> Int8,
            claimed_at_ms -> Nullable<Int8>,
            completed_at_ms -> Nullable<Int8>,
            result -> Nullable<Text>,
            error_code -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.deployment (singleton) {
            singleton -> Bool,
            version -> Int4,
            server_name -> Text,
            issuer -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.devices (id) {
            id -> Text,
            user_id -> Text,
            installation_id -> Text,
            name -> Text,
            session_id -> Text,
            token_hash -> Text,
            generation -> Int8,
            revoked -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.domain_audit (sequence) {
            sequence -> Int8,
            actor_user_id -> Text,
            operation -> Text,
            object_id -> Text,
            at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.domain_commands (actor_user_id, operation, key) {
            actor_user_id -> Text,
            operation -> Text,
            key -> Text,
            digest -> Text,
            agent_id -> Text,
            binding_id -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.domain_deployment (singleton) {
            singleton -> Bool,
            version -> Int4,
            namespace -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.encrypted_dispatches (dispatch_id) {
            dispatch_id -> Text,
            inbox_id -> Text,
            source_digest -> Text,
            projection -> Jsonb,
            seal_digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.encrypted_publications (id) {
            id -> Text,
            dispatch_id -> Text,
            execution_id -> Text,
            request_id -> Text,
            kind -> Text,
            payload -> Jsonb,
            state -> Text,
            matrix_event_id -> Nullable<Text>,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.event_attachments (dispatch_id) {
            dispatch_id -> Text,
            descriptor -> Jsonb,
            descriptor_digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.execution_leases (agent_id) {
            agent_id -> Text,
            owner_user_id -> Text,
            device_id -> Text,
            device_generation -> Int8,
            epoch -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.file_chunks (file_id, byte_offset) {
            file_id -> Text,
            byte_offset -> Int4,
            content -> Bytea,
        }
    }

    diesel::table! {
        hagency_agent_v1.file_delivery_denials (owner_event_id) {
            owner_event_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.file_outbox (id) {
            id -> Text,
            request_id -> Text,
            owner_event_id -> Text,
            execution_id -> Text,
            agent_id -> Text,
            binding_id -> Text,
            owner_user_id -> Text,
            room_id -> Text,
            puppet_mxid -> Text,
            requester_mxid -> Text,
            thread_root -> Text,
            binding_generation -> Int8,
            dispatch_epoch -> Int8,
            dispatch_device_id -> Text,
            manifest -> Jsonb,
            manifest_digest -> Text,
            received_bytes -> Int8,
            content -> Bytea,
            state -> Text,
            mxc_url -> Nullable<Text>,
            worker_phase -> Nullable<Text>,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            matrix_event_id -> Nullable<Text>,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.inbound_transactions (id) {
            id -> Text,
            digest -> Text,
            body -> Jsonb,
            received_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.owner_events (id) {
            id -> Text,
            binding_id -> Text,
            agent_id -> Text,
            owner_user_id -> Text,
            event_id -> Text,
            room_id -> Text,
            requester_mxid -> Text,
            thread_root -> Text,
            body -> Text,
            digest -> Text,
            binding_generation -> Int8,
            state -> Text,
            dispatch_epoch -> Nullable<Int8>,
            dispatch_device_id -> Nullable<Text>,
            execution_id -> Nullable<Text>,
            outcome -> Nullable<Text>,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.pause_notice_outbox (id) {
            id -> Text,
            binding_id -> Text,
            event_id -> Text,
            requester_mxid -> Text,
            digest -> Text,
            binding_generation -> Int8,
            matrix_txn_id -> Text,
            thread_root -> Text,
            created_at_ms -> Int8,
            queued_at_ms -> Int8,
            state -> Text,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            matrix_event_id -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.processing_outbox (id) {
            id -> Text,
            owner_event_id -> Text,
            execution_id -> Text,
            agent_id -> Text,
            binding_id -> Text,
            owner_user_id -> Text,
            room_id -> Text,
            event_id -> Text,
            puppet_mxid -> Text,
            binding_generation -> Int8,
            dispatch_epoch -> Int8,
            dispatch_device_id -> Text,
            content -> Jsonb,
            matrix_txn_id -> Text,
            state -> Text,
            delivery_blocked -> Bool,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            matrix_event_id -> Nullable<Text>,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.projects (id) {
            id -> Text,
            space_id -> Text,
            active -> Bool,
            creation_policy -> Text,
            revision -> Int8,
            archived -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.readiness_receipts (event_id) {
            event_id -> Text,
            received_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.readiness_room (singleton) {
            singleton -> Bool,
            room_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.reply_outbox (id) {
            id -> Text,
            owner_event_id -> Text,
            agent_id -> Text,
            binding_id -> Text,
            owner_user_id -> Text,
            room_id -> Text,
            puppet_mxid -> Text,
            thread_root -> Text,
            body -> Text,
            payload_digest -> Text,
            matrix_txn_id -> Text,
            binding_generation -> Int8,
            dispatch_epoch -> Int8,
            delivery_epoch -> Int8,
            state -> Text,
            delivery_blocked -> Bool,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            matrix_event_id -> Nullable<Text>,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.request_notice_deployment (singleton) {
            singleton -> Bool,
            since_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.request_notice_outbox (id) {
            id -> Text,
            owner_event_id -> Text,
            kind -> Text,
            matrix_txn_id -> Text,
            state -> Text,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            matrix_event_id -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.rooms (room_id) {
            room_id -> Text,
            project_id -> Text,
            active -> Bool,
            creation_policy -> Text,
            revision -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.routing_jobs (transaction_id) {
            transaction_id -> Text,
            state -> Text,
            received_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.routing_rejections (transaction_id, event_id, binding_id) {
            transaction_id -> Text,
            event_id -> Text,
            binding_id -> Text,
            reason -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.scope_pauses (kind, scope_id) {
            kind -> Text,
            scope_id -> Text,
            paused -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.sessions (id) {
            id -> Text,
            user_id -> Text,
            token_hash -> Text,
            client_id -> Text,
            valid_until_ms -> Int8,
            revoked -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.users (id) {
            id -> Text,
            issuer -> Text,
            subject -> Text,
            mxid -> Text,
            active -> Bool,
        }
    }

    diesel::joinable!(agent_crypto_identities -> agents (agent_id));
    diesel::joinable!(agent_crypto_identities -> devices (execution_device_id));
    diesel::joinable!(agent_threads -> bindings (binding_id));
    diesel::joinable!(agents -> users (owner_user_id));
    diesel::joinable!(bindings -> agents (agent_id));
    diesel::joinable!(bindings -> projects (project_id));
    diesel::joinable!(ciphertext_inbox -> agents (agent_id));
    diesel::joinable!(ciphertext_inbox -> bindings (binding_id));
    diesel::joinable!(ciphertext_inbox -> users (owner_user_id));
    diesel::joinable!(configuration_observations -> agents (agent_id));
    diesel::joinable!(configuration_observations -> devices (execution_device_id));
    diesel::joinable!(configuration_presence -> devices (device_id));
    diesel::joinable!(configuration_requests -> agents (agent_id));
    diesel::joinable!(configuration_requests -> users (owner_user_id));
    diesel::joinable!(devices -> users (user_id));
    diesel::joinable!(domain_audit -> users (actor_user_id));
    diesel::joinable!(domain_commands -> agents (agent_id));
    diesel::joinable!(domain_commands -> bindings (binding_id));
    diesel::joinable!(domain_commands -> users (actor_user_id));
    diesel::joinable!(encrypted_dispatches -> ciphertext_inbox (inbox_id));
    diesel::joinable!(encrypted_dispatches -> owner_events (dispatch_id));
    diesel::joinable!(encrypted_publications -> encrypted_dispatches (dispatch_id));
    diesel::joinable!(event_attachments -> owner_events (dispatch_id));
    diesel::joinable!(execution_leases -> agents (agent_id));
    diesel::joinable!(execution_leases -> devices (device_id));
    diesel::joinable!(execution_leases -> users (owner_user_id));
    diesel::joinable!(file_chunks -> file_outbox (file_id));
    diesel::joinable!(file_delivery_denials -> owner_events (owner_event_id));
    diesel::joinable!(file_outbox -> owner_events (owner_event_id));
    diesel::joinable!(owner_events -> agents (agent_id));
    diesel::joinable!(owner_events -> bindings (binding_id));
    diesel::joinable!(owner_events -> devices (dispatch_device_id));
    diesel::joinable!(owner_events -> users (owner_user_id));
    diesel::joinable!(pause_notice_outbox -> bindings (binding_id));
    diesel::joinable!(processing_outbox -> agents (agent_id));
    diesel::joinable!(processing_outbox -> bindings (binding_id));
    diesel::joinable!(processing_outbox -> devices (dispatch_device_id));
    diesel::joinable!(processing_outbox -> owner_events (owner_event_id));
    diesel::joinable!(processing_outbox -> users (owner_user_id));
    diesel::joinable!(reply_outbox -> agents (agent_id));
    diesel::joinable!(reply_outbox -> bindings (binding_id));
    diesel::joinable!(reply_outbox -> owner_events (owner_event_id));
    diesel::joinable!(reply_outbox -> users (owner_user_id));
    diesel::joinable!(request_notice_outbox -> owner_events (owner_event_id));
    diesel::joinable!(rooms -> projects (project_id));
    diesel::joinable!(routing_jobs -> inbound_transactions (transaction_id));
    diesel::joinable!(routing_rejections -> inbound_transactions (transaction_id));
    diesel::joinable!(sessions -> users (user_id));

    diesel::allow_tables_to_appear_in_same_query!(
        agent_crypto_identities,
        agent_threads,
        agents,
        bindings,
        ciphertext_inbox,
        configuration_observations,
        configuration_presence,
        configuration_requests,
        deployment,
        devices,
        domain_audit,
        domain_commands,
        domain_deployment,
        encrypted_dispatches,
        encrypted_publications,
        event_attachments,
        execution_leases,
        file_chunks,
        file_delivery_denials,
        file_outbox,
        inbound_transactions,
        owner_events,
        pause_notice_outbox,
        processing_outbox,
        projects,
        readiness_receipts,
        readiness_room,
        reply_outbox,
        request_notice_deployment,
        request_notice_outbox,
        rooms,
        routing_jobs,
        routing_rejections,
        scope_pauses,
        sessions,
        users,
    );
}
