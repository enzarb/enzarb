// TS mirror of agent/src/acp/events.rs — keep in sync with the Rust enums.

export interface SessionModeInfo {
	id: string;
	name: string;
	description: string | null;
}

export interface ConfigValueInfo {
	value: string;
	name: string;
}

export interface ConfigOptionInfo {
	id: string;
	name: string;
	category: string | null;
	current_value: string;
	options: ConfigValueInfo[];
}

export interface SessionMeta {
	id: string;
	label: string;
	provider: string;
	cwd: string;
	updated_at: string | null;
	status: 'live' | 'idle';
	mode_id: string | null;
	available_modes: SessionModeInfo[];
	config_options: ConfigOptionInfo[];
	archived: boolean;
	_meta?: Record<string, unknown>;
}

/** Mirrors agent/src/acp/providers.rs::ProviderSpec, served from GET /agent/providers. */
export interface ProviderInfo {
	id: string;
	display_name: string;
	spawn_command: string;
}

export interface DiffPayload {
	path: string;
	old_text: string | null;
	new_text: string;
}

export interface PlanEntryPayload {
	content: string;
	priority: string;
	status: string;
}

export interface PermissionOptionPayload {
	option_id: string;
	label: string;
	kind: 'allow_once' | 'allow_always' | 'reject_once' | 'reject_always';
}

export interface AvailableCommandInfo {
	name: string;
	description: string;
}

/** Every event carries this from the WS envelope (agent/src/external/agent.rs::encode). */
export interface WithTimestamp {
	ts_ms: number;
}

export type AcpWsEvent = WithTimestamp &
	(
		| { type: 'session_list'; sessions: SessionMeta[] }
	| { type: 'session_created'; session: SessionMeta }
	| { type: 'message_chunk'; session_id: string; role: 'user' | 'assistant'; text: string }
	| { type: 'thought_chunk'; session_id: string; text: string }
	| {
			type: 'tool_call_created';
			session_id: string;
			tool_call_id: string;
			kind: string;
			title: string;
			status: string;
			path: string | null;
			plan: string | null;
			command: string | null;
			input?: unknown;
	  }
	| {
			type: 'tool_call_updated';
			session_id: string;
			tool_call_id: string;
			status: string | null;
			diff: DiffPayload | null;
			output: string | null;
			path: string | null;
			plan: string | null;
			command: string | null;
			input?: unknown;
	  }
	| { type: 'plan_update'; session_id: string; entries: PlanEntryPayload[] }
	| {
			type: 'permission_request';
			session_id: string;
			request_id: string;
			tool_call_id: string;
			title: string;
			options: PermissionOptionPayload[];
			plan: string | null;
	  }
	| { type: 'permission_resolved'; session_id: string; request_id: string }
	| { type: 'mode_changed'; session_id: string; mode_id: string }
	| { type: 'config_options_changed'; session_id: string; config_options: ConfigOptionInfo[] }
	| {
			type: 'session_state';
			session_id: string;
			mode_id: string | null;
			available_modes: SessionModeInfo[];
			config_options: ConfigOptionInfo[];
	  }
	| { type: 'error'; session_id: string | null; message: string }
	| { type: 'turn_status'; session_id: string; running: boolean }
	| { type: 'turn_ended'; session_id: string; stop_reason: string }
	| {
			type: 'usage_update';
			session_id: string;
			used: number;
			size: number;
			cost_amount: number | null;
			cost_currency: string | null;
	  }
	| { type: 'available_commands_update'; session_id: string; commands: AvailableCommandInfo[] }
	| { type: 'session_info_update'; session_id: string; title: string | null }
	);

export type AcpWsClientMsg =
	| { type: 'send_message'; text: string }
	| { type: 'permission_response'; request_id: string; option_id: string }
	| { type: 'set_permission_mode'; mode_id: string }
	| { type: 'set_config_option'; config_id: string; value: string }
	| { type: 'cancel' };
