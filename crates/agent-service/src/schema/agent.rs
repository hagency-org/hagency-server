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
            execution_kind -> Text,
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
        hagency_agent_v1.executor_matrix_devices (scope_id, device_id) {
            scope_id -> Text,
            puppet_mxid -> Text,
            device_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            kind -> Text,
            revoked -> Bool,
            retiring -> Bool,
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
        hagency_agent_v1.management_agent_budget_cancellations (command_id) {
            command_id -> Text,
            human_command_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_budget_commands (id) {
            id -> Text,
            topup_id -> Text,
            request_id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            payload -> Text,
            digest -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_budget_issuance (command_id) {
            command_id -> Text,
            executor_generation -> Int8,
            issued_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_budget_results (command_id) {
            command_id -> Text,
            payload -> Text,
            digest -> Text,
            ledger_revision -> Int8,
            state -> Text,
            budget_revision -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_budget_tickets (command_id, token_hash) {
            command_id -> Text,
            token_hash -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            purpose -> Text,
            issued_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_decisions (request_id) {
            request_id -> Text,
            request_revision -> Int8,
            administrator_user_id -> Text,
            authority_revision -> Int8,
            command_id -> Text,
            digest -> Text,
            decision -> Text,
            decided_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_executor_commands (id) {
            id -> Text,
            intent_id -> Text,
            operation -> Text,
            created_at_ms -> Int8,
            state -> Text,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
            receipt -> Nullable<Text>,
            receipt_digest -> Nullable<Text>,
            command_payload -> Nullable<Text>,
            period_key -> Nullable<Text>,
            receipt_ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_intents (id) {
            id -> Text,
            request_id -> Text,
            executor_owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            payload -> Text,
            digest -> Text,
            state -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_matrix (assignment_id) {
            assignment_id -> Text,
            request_id -> Text,
            agent_id -> Text,
            puppet_mxid -> Text,
            room_id -> Text,
            space_id -> Text,
            state -> Text,
            challenge_id -> Nullable<Text>,
            native_until_ms -> Int8,
            effect_started -> Bool,
            verified_at_ms -> Nullable<Int8>,
            cleanup_observed_at_ms -> Nullable<Int8>,
            blocking_reason -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_matrix_checks (id) {
            id -> Text,
            assignment_id -> Text,
            payload -> Text,
            digest -> Text,
            expires_at_ms -> Int8,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
            observation -> Nullable<Text>,
            observation_digest -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_outbox (id) {
            id -> Text,
            request_id -> Text,
            request_revision -> Int8,
            recipient_mxid -> Text,
            transaction_id -> Text,
            state -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_reallocations (request_id) {
            request_id -> Text,
            previous_request_id -> Text,
            allocation_generation -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_recurring_terms (request_id) {
            request_id -> Text,
            calendar -> Text,
            renewal_mode -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_renewal_cursors (executor_device_id) {
            executor_device_id -> Text,
            after_intent_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_requests (id) {
            id -> Text,
            plan_id -> Nullable<Text>,
            setup_id -> Nullable<Text>,
            setup_binding_revision -> Int8,
            allocation_id -> Nullable<Text>,
            allocation_receipt_digest -> Text,
            manager_user_id -> Text,
            domain_project_id -> Nullable<Text>,
            room_id -> Text,
            proposed_agent_id -> Text,
            definition -> Text,
            definition_digest -> Text,
            requested_tokens -> Int8,
            period_key -> Text,
            administrator_user_id -> Text,
            authority_revision -> Int8,
            state -> Text,
            revision -> Int8,
            created_at_ms -> Int8,
            expires_at_ms -> Int8,
            pool_contribution_id -> Nullable<Text>,
            pool_grant_id -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_runtime_commands (id) {
            id -> Text,
            assignment_id -> Text,
            operation -> Text,
            created_at_ms -> Int8,
            payload -> Nullable<Text>,
            digest -> Nullable<Text>,
            expires_at_ms -> Nullable<Int8>,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_runtime_observations (id) {
            id -> Text,
            command_id -> Text,
            payload -> Text,
            digest -> Text,
            ledger_revision -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_settlement_commands (id) {
            id -> Text,
            request_id -> Text,
            fence_id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            payload -> Text,
            digest -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_settlement_issuance (command_id) {
            command_id -> Text,
            issued_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_settlement_results (command_id) {
            command_id -> Text,
            payload -> Text,
            digest -> Text,
            ledger_revision -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_settlement_tickets (command_id, token_hash) {
            command_id -> Text,
            token_hash -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            issued_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_agent_topups (id) {
            id -> Text,
            request_id -> Text,
            assignment_id -> Text,
            manager_user_id -> Text,
            payload -> Text,
            digest -> Text,
            expected_budget_revision -> Int8,
            expected_budget_digest -> Text,
            expected_ceiling_tokens -> Int8,
            add_tokens -> Int8,
            purpose -> Text,
            state -> Text,
            revision -> Int8,
            created_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_allocations (id) {
            id -> Text,
            project_id -> Text,
            project_revision -> Int8,
            grant_id -> Text,
            request_id -> Text,
            state -> Text,
            revision -> Int8,
            expires_at_ms -> Int8,
            reserve_command_id -> Nullable<Text>,
            release_command_id -> Nullable<Text>,
            result -> Nullable<Text>,
            balance -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_commands (actor_user_id, device_id, device_generation, command_id) {
            actor_user_id -> Text,
            device_id -> Text,
            device_generation -> Int8,
            command_id -> Text,
            operation -> Text,
            digest -> Text,
            result -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_contributions (id) {
            id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            resource_id -> Text,
            resource_revision -> Text,
            label -> Text,
            runtime -> Text,
            capabilities -> Text,
            coordinator_mxid -> Text,
            state -> Text,
            revision -> Int8,
            request_id -> Text,
            created_at_ms -> Int8,
            updated_at_ms -> Int8,
            publication_first -> Bool,
            ledger_replaced_epoch -> Nullable<Int8>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_deployment (singleton) {
            singleton -> Bool,
            version -> Int4,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_executor_commands (id) {
            id -> Text,
            allocation_id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            operation -> Text,
            command -> Text,
            digest -> Text,
            state -> Text,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
            receipt -> Nullable<Text>,
            receipt_digest -> Nullable<Text>,
            receipt_ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_executor_ledgers (owner_user_id, executor_device_id, epoch) {
            owner_user_id -> Text,
            executor_device_id -> Text,
            ledger_id -> Text,
            epoch -> Int8,
            declared_at_ms -> Int8,
            swept -> Bool,
            sweep -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_grant_publication_tombstones (owner_user_id, executor_device_id, command_id) {
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            command_id -> Text,
            operation -> Text,
            payload_digest -> Text,
            publication_digest -> Text,
            receipt_digest -> Text,
            observed_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_outbox (id) {
            id -> Text,
            request_id -> Text,
            request_revision -> Int8,
            recipient_mxid -> Text,
            state -> Text,
            transaction_id -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_owner_coordination_merges (contribution_id) {
            contribution_id -> Text,
            previous_contribution -> Jsonb,
            previous_allocation_requests -> Jsonb,
            merged_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_parent_commands (id) {
            id -> Text,
            grant_id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            operation -> Text,
            command -> Text,
            digest -> Text,
            state -> Text,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
            receipt -> Nullable<Text>,
            receipt_digest -> Nullable<Text>,
            receipt_ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_parent_settlements (grant_id) {
            grant_id -> Text,
            revision -> Int8,
            state -> Text,
            close_command_id -> Text,
            latest_command_id -> Text,
            receipt -> Nullable<Text>,
            receipt_digest -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_project_authorities (plan_id) {
            plan_id -> Text,
            administrator_user_id -> Text,
            revision -> Int8,
            assigned_by_user_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_project_role_merges (plan_id) {
            plan_id -> Text,
            manager_user_id -> Text,
            previous_authority -> Nullable<Jsonb>,
            merged_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_project_setups (id) {
            id -> Text,
            plan_id -> Text,
            allocation_id -> Text,
            manager_user_id -> Text,
            plan_revision -> Int8,
            allocation_receipt_digest -> Text,
            authority_revision -> Int8,
            definition_digest -> Text,
            mode -> Text,
            state -> Text,
            revision -> Int8,
            name -> Text,
            manager_mxid -> Text,
            administrator_mxid -> Text,
            space_creation_command_id -> Text,
            room_creation_command_id -> Text,
            space_id -> Nullable<Text>,
            room_id -> Nullable<Text>,
            domain_project_id -> Nullable<Text>,
            verified_at_ms -> Nullable<Int8>,
            current_binding_revision -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_projects (id) {
            id -> Text,
            manager_user_id -> Text,
            contribution_id -> Text,
            name -> Text,
            purpose -> Text,
            requested_tokens -> Int8,
            state -> Text,
            revision -> Int8,
            request_id -> Text,
            created_at_ms -> Int8,
            updated_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_requests (id) {
            id -> Text,
            kind -> Text,
            contribution_id -> Text,
            requester_user_id -> Text,
            approver_mxid -> Text,
            state -> Text,
            revision -> Int8,
            created_at_ms -> Int8,
            expires_at_ms -> Int8,
            decided_at_ms -> Nullable<Int8>,
            project_id -> Nullable<Text>,
            allocation_id -> Nullable<Text>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_resource_grants (id) {
            id -> Text,
            contribution_id -> Text,
            owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            receipt -> Text,
            receipt_digest -> Text,
            state -> Text,
            revision -> Int8,
            period_key -> Text,
            expires_at_ms -> Int8,
            observed_at_ms -> Int8,
            chain_root_id -> Text,
            predecessor_grant_id -> Nullable<Text>,
            succession -> Nullable<Text>,
            period_start_ms -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_setup_bindings (setup_id, revision) {
            setup_id -> Text,
            revision -> Int8,
            allocation_id -> Text,
            allocation_receipt_digest -> Text,
            plan_revision -> Int8,
            authority_revision -> Int8,
            administrator_mxid -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_activity_renewals (id) {
            id -> Text,
            runtime_id -> Text,
            from_epoch -> Int8,
            to_epoch -> Int8,
            expires_at_ms -> Int8,
            authority_hash -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_approvals (id) {
            id -> Text,
            work_id -> Text,
            runtime_id -> Text,
            owner_user_id -> Text,
            requester_mxid -> Text,
            room_id -> Text,
            payload -> Text,
            digest -> Text,
            expires_at_ms -> Int8,
            decision -> Nullable<Text>,
            decision_by_owner -> Nullable<Bool>,
            decided_by_mxid -> Nullable<Text>,
            decided_at_ms -> Nullable<Int8>,
            consumed -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_assignments (id) {
            id -> Text,
            request_id -> Text,
            agent_id -> Text,
            manager_user_id -> Text,
            executor_owner_user_id -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            agent_reservation_id -> Text,
            reserve_receipt_digest -> Text,
            state -> Text,
            generation -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_attempts (id) {
            id -> Text,
            assignment_id -> Text,
            runtime_id -> Text,
            predecessor_id -> Text,
            original_runtime_id -> Text,
            process_attempt -> Int8,
            input_payload -> Text,
            input_digest -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_attempts (runtime_id) {
            runtime_id -> Text,
            assignment_id -> Text,
            budget_revision -> Int8,
            budget_digest -> Text,
            applied_command_id -> Text,
            pin_payload -> Text,
            pin_digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_demand_bindings (demand_id) {
            demand_id -> Text,
            work_id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
            budget_revision -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_demand_blocks (demand_id, budget_revision) {
            demand_id -> Text,
            budget_revision -> Int8,
            reason -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_demand_heads (demand_id, budget_revision) {
            demand_id -> Text,
            budget_revision -> Int8,
            budget_digest -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_demand_starts (demand_id) {
            demand_id -> Text,
            start_intent_id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
            budget_revision -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_budget_demand_terminals (demand_id) {
            demand_id -> Text,
            reason -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_context_sequences (assignment_id, room_id, thread_root) {
            assignment_id -> Text,
            room_id -> Text,
            thread_root -> Text,
            ordinal -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_context_starts (work_id) {
            work_id -> Text,
            context_digest -> Text,
            input_digest -> Text,
            activity_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_contexts (work_id) {
            work_id -> Text,
            assignment_id -> Text,
            room_id -> Text,
            thread_root -> Text,
            ordinal -> Int8,
            payload -> Text,
            digest -> Text,
            input_digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_control_commands (actor_user_id, device_id, command_id) {
            actor_user_id -> Text,
            device_id -> Text,
            command_id -> Text,
            assignment_id -> Text,
            input_digest -> Text,
            result -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_controls (assignment_id) {
            assignment_id -> Text,
            revision -> Int8,
            mode -> Text,
            running_since_ms -> Nullable<Int8>,
            updated_at_ms -> Int8,
            origin -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demand_bindings (demand_id) {
            demand_id -> Text,
            work_id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demand_blocks (demand_id) {
            demand_id -> Text,
            reason -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demand_retries (demand_id, retry) {
            demand_id -> Text,
            retry -> Int8,
            previous_runtime_id -> Text,
            receipt_id -> Text,
            created_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demand_retry_starts (demand_id, retry) {
            demand_id -> Text,
            retry -> Int8,
            start_intent_id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demand_starts (demand_id) {
            demand_id -> Text,
            start_intent_id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_demands (id) {
            id -> Text,
            intake_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_e2ee_candidates (assignment_id, event_id) {
            assignment_id -> Text,
            event_id -> Text,
            payload -> Text,
            received_at_ms -> Int8,
            state -> Text,
            last_polled_ms -> Int8,
            room_binding_id -> Nullable<Text>,
            room_binding_generation -> Nullable<Int8>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_event_pins (assignment_id, event_id) {
            assignment_id -> Text,
            event_id -> Text,
            work_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_event_revisions (revision_event_id) {
            revision_event_id -> Text,
            room_id -> Text,
            target_event_id -> Text,
            kind -> Text,
            sender_mxid -> Text,
            body -> Nullable<Text>,
            body_digest -> Nullable<Text>,
            received_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_executor_protocols (device_id) {
            device_id -> Text,
            owner_user_id -> Text,
            generation -> Int8,
            session_id -> Text,
            qualified -> Bool,
            observed_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_input_modes (work_id) {
            work_id -> Text,
            mode -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_intake (id) {
            id -> Text,
            assignment_id -> Text,
            event_id -> Text,
            event_payload -> Text,
            event_digest -> Text,
            control_revision -> Int8,
            received_at_ms -> Int8,
            expires_at_ms -> Int8,
            state -> Text,
            reason -> Nullable<Text>,
            room_binding_id -> Nullable<Text>,
            room_binding_generation -> Nullable<Int8>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_process_receipts (id) {
            id -> Text,
            runtime_id -> Text,
            payload -> Text,
            digest -> Text,
            ledger_revision -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_replies (work_id) {
            work_id -> Text,
            content -> Text,
            content_digest -> Text,
            matrix_txn_id -> Text,
            created_at_ms -> Int8,
            state -> Text,
            matrix_event_id -> Nullable<Text>,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
            sent_at_ms -> Nullable<Int8>,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_reply_deliveries (work_id) {
            work_id -> Text,
            payload -> Text,
            digest -> Text,
            matrix_content -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_result_tickets (runtime_id, token_hash) {
            runtime_id -> Text,
            token_hash -> Text,
            executor_device_id -> Text,
            executor_generation -> Int8,
            mode -> Text,
            issued_at_ms -> Int8,
            expires_at_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_runtimes (id) {
            id -> Text,
            assignment_id -> Text,
            payload -> Text,
            digest -> Text,
            created_at_ms -> Int8,
            expires_at_ms -> Int8,
            state -> Text,
            permanent_fence -> Bool,
            process_digest -> Nullable<Text>,
            lease_hash -> Nullable<Text>,
            lease_mode -> Text,
            lease_until_ms -> Int8,
            activity_epoch -> Int8,
            activity_hash -> Nullable<Text>,
            activity_until_ms -> Int8,
            last_renewal_id -> Nullable<Text>,
            last_renewal_epoch -> Nullable<Int8>,
            issuance_void -> Bool,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_work (id) {
            id -> Text,
            runtime_id -> Text,
            event_id -> Text,
            payload -> Text,
            digest -> Text,
            created_at_ms -> Int8,
            state -> Text,
            acknowledged_at_ms -> Nullable<Int8>,
            started_at_ms -> Nullable<Int8>,
            completed_at_ms -> Nullable<Int8>,
            start_epoch -> Nullable<Int8>,
            call_id -> Nullable<Text>,
            latest_receipt -> Nullable<Text>,
            known_consumed_tokens -> Int8,
            retained_tokens -> Nullable<Int8>,
            usage_complete -> Bool,
            lease_hash -> Nullable<Text>,
            lease_until_ms -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_work_receipts (id) {
            id -> Text,
            work_id -> Text,
            payload -> Text,
            digest -> Text,
            ledger_revision -> Int8,
            ledger_epoch -> Int8,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_shared_work_refusals (work_id) {
            work_id -> Text,
            payload -> Text,
            digest -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.management_verifications (contribution_id) {
            contribution_id -> Text,
            contribution_revision -> Int8,
            executor_generation -> Int8,
            resource_revision -> Text,
            verified_at_ms -> Int8,
            fresh_until_ms -> Int8,
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
        hagency_agent_v1.personal_e2ee_candidates (binding_id, event_id) {
            binding_id -> Text,
            event_id -> Text,
            payload -> Text,
            received_at_ms -> Int8,
            done -> Bool,
            last_polled_ms -> Int8,
            confirmation -> Nullable<Text>,
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
        hagency_agent_v1.progress_status (id) {
            id -> Text,
            owner_event_id -> Text,
            execution_id -> Text,
            binding_id -> Text,
            binding_generation -> Int8,
            started_at_ms -> Int8,
            recent -> Jsonb,
            step_count -> Int8,
            final_body -> Nullable<Text>,
            revision -> Int8,
            sent_revision -> Int8,
            matrix_txn_id -> Text,
            status_event_id -> Nullable<Text>,
            state -> Text,
            worker_token -> Nullable<Text>,
            worker_until_ms -> Int8,
            last_sent_ms -> Int8,
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
        hagency_agent_v1.rooms (room_id, project_id) {
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
        hagency_agent_v1.shared_agent_room_commands (actor_user_id, key) {
            actor_user_id -> Text,
            key -> Text,
            digest -> Text,
            binding_id -> Text,
        }
    }

    diesel::table! {
        hagency_agent_v1.shared_agent_rooms (id) {
            id -> Text,
            assignment_id -> Text,
            agent_id -> Text,
            project_id -> Text,
            room_id -> Text,
            state -> Text,
            generation -> Int8,
            departure_reason -> Nullable<Text>,
            restoration_requires_membership -> Bool,
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
    diesel::joinable!(management_agent_budget_cancellations -> management_agent_budget_commands (command_id));
    diesel::joinable!(management_agent_budget_commands -> devices (executor_device_id));
    diesel::joinable!(management_agent_budget_commands -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_budget_commands -> management_agent_topups (topup_id));
    diesel::joinable!(management_agent_budget_commands -> users (owner_user_id));
    diesel::joinable!(management_agent_budget_issuance -> management_agent_budget_commands (command_id));
    diesel::joinable!(management_agent_budget_results -> management_agent_budget_commands (command_id));
    diesel::joinable!(management_agent_budget_tickets -> devices (executor_device_id));
    diesel::joinable!(management_agent_budget_tickets -> management_agent_budget_commands (command_id));
    diesel::joinable!(management_agent_decisions -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_decisions -> users (administrator_user_id));
    diesel::joinable!(management_agent_executor_commands -> management_agent_intents (intent_id));
    diesel::joinable!(management_agent_intents -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_intents -> users (executor_owner_user_id));
    diesel::joinable!(management_agent_matrix -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_agent_matrix_checks -> management_agent_matrix (assignment_id));
    diesel::joinable!(management_agent_outbox -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_recurring_terms -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_renewal_cursors -> devices (executor_device_id));
    diesel::joinable!(management_agent_requests -> management_allocations (allocation_id));
    diesel::joinable!(management_agent_requests -> management_contributions (pool_contribution_id));
    diesel::joinable!(management_agent_requests -> management_project_setups (setup_id));
    diesel::joinable!(management_agent_requests -> management_projects (plan_id));
    diesel::joinable!(management_agent_requests -> management_resource_grants (pool_grant_id));
    diesel::joinable!(management_agent_requests -> projects (domain_project_id));
    diesel::joinable!(management_agent_runtime_commands -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_agent_runtime_observations -> management_agent_runtime_commands (command_id));
    diesel::joinable!(management_agent_settlement_commands -> management_agent_executor_commands (fence_id));
    diesel::joinable!(management_agent_settlement_commands -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_settlement_commands -> users (owner_user_id));
    diesel::joinable!(management_agent_settlement_issuance -> management_agent_settlement_commands (command_id));
    diesel::joinable!(management_agent_settlement_results -> management_agent_settlement_commands (command_id));
    diesel::joinable!(management_agent_settlement_tickets -> devices (executor_device_id));
    diesel::joinable!(management_agent_settlement_tickets -> management_agent_settlement_commands (command_id));
    diesel::joinable!(management_agent_topups -> management_agent_requests (request_id));
    diesel::joinable!(management_agent_topups -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_agent_topups -> users (manager_user_id));
    diesel::joinable!(management_allocations -> management_projects (project_id));
    diesel::joinable!(management_allocations -> management_resource_grants (grant_id));
    diesel::joinable!(management_commands -> users (actor_user_id));
    diesel::joinable!(management_executor_commands -> management_allocations (allocation_id));
    diesel::joinable!(management_executor_commands -> users (owner_user_id));
    diesel::joinable!(management_grant_publication_tombstones -> devices (executor_device_id));
    diesel::joinable!(management_grant_publication_tombstones -> users (owner_user_id));
    diesel::joinable!(management_outbox -> management_requests (request_id));
    diesel::joinable!(management_owner_coordination_merges -> management_contributions (contribution_id));
    diesel::joinable!(management_parent_commands -> users (owner_user_id));
    diesel::joinable!(management_parent_settlements -> management_resource_grants (grant_id));
    diesel::joinable!(management_project_role_merges -> management_projects (plan_id));
    diesel::joinable!(management_project_role_merges -> users (manager_user_id));
    diesel::joinable!(management_project_setups -> management_allocations (allocation_id));
    diesel::joinable!(management_project_setups -> management_projects (plan_id));
    diesel::joinable!(management_project_setups -> projects (domain_project_id));
    diesel::joinable!(management_project_setups -> users (manager_user_id));
    diesel::joinable!(management_projects -> management_contributions (contribution_id));
    diesel::joinable!(management_projects -> users (manager_user_id));
    diesel::joinable!(management_requests -> users (requester_user_id));
    diesel::joinable!(management_resource_grants -> management_contributions (contribution_id));
    diesel::joinable!(management_resource_grants -> users (owner_user_id));
    diesel::joinable!(management_setup_bindings -> management_allocations (allocation_id));
    diesel::joinable!(management_shared_activity_renewals -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_approvals -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_approvals -> management_shared_work (work_id));
    diesel::joinable!(management_shared_approvals -> users (owner_user_id));
    diesel::joinable!(management_shared_assignments -> management_agent_requests (request_id));
    diesel::joinable!(management_shared_attempts -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_budget_attempts -> management_agent_budget_results (applied_command_id));
    diesel::joinable!(management_shared_budget_attempts -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_budget_attempts -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_budget_demand_bindings -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_budget_demand_bindings -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_budget_demand_bindings -> management_shared_work (work_id));
    diesel::joinable!(management_shared_budget_demand_blocks -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_budget_demand_heads -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_budget_demand_starts -> management_shared_attempts (start_intent_id));
    diesel::joinable!(management_shared_budget_demand_starts -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_budget_demand_starts -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_budget_demand_terminals -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_context_sequences -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_context_starts -> management_shared_contexts (work_id));
    diesel::joinable!(management_shared_contexts -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_contexts -> management_shared_work (work_id));
    diesel::joinable!(management_shared_control_commands -> management_shared_controls (assignment_id));
    diesel::joinable!(management_shared_control_commands -> users (actor_user_id));
    diesel::joinable!(management_shared_controls -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_demand_bindings -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_demand_bindings -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_demand_bindings -> management_shared_work (work_id));
    diesel::joinable!(management_shared_demand_blocks -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_demand_retries -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_demand_retries -> management_shared_process_receipts (receipt_id));
    diesel::joinable!(management_shared_demand_retries -> management_shared_runtimes (previous_runtime_id));
    diesel::joinable!(management_shared_demand_retry_starts -> management_shared_attempts (start_intent_id));
    diesel::joinable!(management_shared_demand_retry_starts -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_demand_starts -> management_shared_attempts (start_intent_id));
    diesel::joinable!(management_shared_demand_starts -> management_shared_demands (demand_id));
    diesel::joinable!(management_shared_demand_starts -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_demands -> management_shared_intake (intake_id));
    diesel::joinable!(management_shared_e2ee_candidates -> shared_agent_rooms (room_binding_id));
    diesel::joinable!(management_shared_event_pins -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_event_pins -> management_shared_work (work_id));
    diesel::joinable!(management_shared_input_modes -> management_shared_work (work_id));
    diesel::joinable!(management_shared_intake -> management_shared_controls (assignment_id));
    diesel::joinable!(management_shared_intake -> shared_agent_rooms (room_binding_id));
    diesel::joinable!(management_shared_process_receipts -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_replies -> management_shared_work (work_id));
    diesel::joinable!(management_shared_reply_deliveries -> management_shared_replies (work_id));
    diesel::joinable!(management_shared_result_tickets -> devices (executor_device_id));
    diesel::joinable!(management_shared_result_tickets -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_runtimes -> management_shared_assignments (assignment_id));
    diesel::joinable!(management_shared_work -> management_shared_runtimes (runtime_id));
    diesel::joinable!(management_shared_work_receipts -> management_shared_work (work_id));
    diesel::joinable!(management_shared_work_refusals -> management_shared_work (work_id));
    diesel::joinable!(management_verifications -> management_contributions (contribution_id));
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
    diesel::joinable!(progress_status -> owner_events (owner_event_id));
    diesel::joinable!(reply_outbox -> agents (agent_id));
    diesel::joinable!(reply_outbox -> bindings (binding_id));
    diesel::joinable!(reply_outbox -> owner_events (owner_event_id));
    diesel::joinable!(reply_outbox -> users (owner_user_id));
    diesel::joinable!(request_notice_outbox -> owner_events (owner_event_id));
    diesel::joinable!(rooms -> projects (project_id));
    diesel::joinable!(routing_jobs -> inbound_transactions (transaction_id));
    diesel::joinable!(routing_rejections -> inbound_transactions (transaction_id));
    diesel::joinable!(sessions -> users (user_id));
    diesel::joinable!(shared_agent_room_commands -> shared_agent_rooms (binding_id));
    diesel::joinable!(shared_agent_room_commands -> users (actor_user_id));
    diesel::joinable!(shared_agent_rooms -> agents (agent_id));
    diesel::joinable!(shared_agent_rooms -> management_shared_assignments (assignment_id));

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
        executor_matrix_devices,
        file_chunks,
        file_delivery_denials,
        file_outbox,
        inbound_transactions,
        management_agent_budget_cancellations,
        management_agent_budget_commands,
        management_agent_budget_issuance,
        management_agent_budget_results,
        management_agent_budget_tickets,
        management_agent_decisions,
        management_agent_executor_commands,
        management_agent_intents,
        management_agent_matrix,
        management_agent_matrix_checks,
        management_agent_outbox,
        management_agent_reallocations,
        management_agent_recurring_terms,
        management_agent_renewal_cursors,
        management_agent_requests,
        management_agent_runtime_commands,
        management_agent_runtime_observations,
        management_agent_settlement_commands,
        management_agent_settlement_issuance,
        management_agent_settlement_results,
        management_agent_settlement_tickets,
        management_agent_topups,
        management_allocations,
        management_commands,
        management_contributions,
        management_deployment,
        management_executor_commands,
        management_executor_ledgers,
        management_grant_publication_tombstones,
        management_outbox,
        management_owner_coordination_merges,
        management_parent_commands,
        management_parent_settlements,
        management_project_authorities,
        management_project_role_merges,
        management_project_setups,
        management_projects,
        management_requests,
        management_resource_grants,
        management_setup_bindings,
        management_shared_activity_renewals,
        management_shared_approvals,
        management_shared_assignments,
        management_shared_attempts,
        management_shared_budget_attempts,
        management_shared_budget_demand_bindings,
        management_shared_budget_demand_blocks,
        management_shared_budget_demand_heads,
        management_shared_budget_demand_starts,
        management_shared_budget_demand_terminals,
        management_shared_context_sequences,
        management_shared_context_starts,
        management_shared_contexts,
        management_shared_control_commands,
        management_shared_controls,
        management_shared_demand_bindings,
        management_shared_demand_blocks,
        management_shared_demand_retries,
        management_shared_demand_retry_starts,
        management_shared_demand_starts,
        management_shared_demands,
        management_shared_e2ee_candidates,
        management_shared_event_pins,
        management_shared_event_revisions,
        management_shared_executor_protocols,
        management_shared_input_modes,
        management_shared_intake,
        management_shared_process_receipts,
        management_shared_replies,
        management_shared_reply_deliveries,
        management_shared_result_tickets,
        management_shared_runtimes,
        management_shared_work,
        management_shared_work_receipts,
        management_shared_work_refusals,
        management_verifications,
        owner_events,
        pause_notice_outbox,
        personal_e2ee_candidates,
        processing_outbox,
        progress_status,
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
        shared_agent_room_commands,
        shared_agent_rooms,
        users,
    );
}
