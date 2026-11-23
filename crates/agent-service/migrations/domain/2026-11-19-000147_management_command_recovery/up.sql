CREATE INDEX IF NOT EXISTS management_command_recovery ON hagency_agent_v1.management_commands(actor_user_id,device_id,command_id);
