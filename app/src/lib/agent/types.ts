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
	cwd: string;
	updated_at: string | null;
	status: 'live' | 'idle';
	mode_id: string | null;
	available_modes: SessionModeInfo[];
	config_options: ConfigOptionInfo[];
	_meta?: Record<string, unknown>;
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

export type AcpWsEvent =
	| { type: 'session_list'; sessions: SessionMeta[] }
	| { type: 'session_created'; session: SessionMeta }
	| { type: 'message_chunk'; session_id: string; role: 'user' | 'assistant'; text: string }
	| {
			type: 'tool_call_created';
			session_id: string;
			tool_call_id: string;
			kind: string;
			title: string;
			status: string;
			plan: string | null;
	  }
	| {
			type: 'tool_call_updated';
			session_id: string;
			tool_call_id: string;
			status: string | null;
			diff: DiffPayload | null;
			output: string | null;
			plan: string | null;
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
	| { type: 'error'; session_id: string | null; message: string };

export type AcpWsClientMsg =
	| { type: 'send_message'; text: string }
	| { type: 'permission_response'; request_id: string; option_id: string }
	| { type: 'set_permission_mode'; mode_id: string }
	| { type: 'set_config_option'; config_id: string; value: string }
	| { type: 'cancel' };
