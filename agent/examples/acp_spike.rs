//! Spike: validates the agent-client-protocol crate API against a real
//! `claude-code-acp` process (npx-launched). Not part of the shipped binary.
//!
//! Requires ANTHROPIC_API_KEY in the environment and network access to fetch
//! the npm package on first run.
//!
//! Usage: cargo run --example acp_spike -- "list the files in this directory"

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, ConnectionTo};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Say hello in one sentence.".to_string());

    // @zed-industries/claude-code-acp was renamed to @agentclientprotocol/claude-agent-acp;
    // the binary name changed too, so AcpAgent::zed_claude_code() (which still shells out to
    // the deprecated npx command) can't be used here.
    let agent = AcpAgent::from_str("npx -y @agentclientprotocol/claude-agent-acp@latest")?;

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                println!("update: {:?}", notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                // Spike only: auto-approve everything to prove the round trip works.
                // Real integration applies the read/edit/execute policy from acp/permissions.rs.
                println!("permission request: {request:?}");
                let option_id = request.options.first().map(|opt| opt.option_id.clone());
                match option_id {
                    Some(id) => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                    )),
                    None => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    )),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            println!("initialized: {:?}", init.agent_info);

            let cwd = std::env::current_dir().map_err(anyhow::Error::from)?;
            let session = connection
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            println!("session created: {:?}", session.session_id);

            let response = connection
                .send_request(PromptRequest::new(
                    session.session_id,
                    vec![ContentBlock::Text(TextContent::new(prompt))],
                ))
                .block_task()
                .await?;
            println!("stop reason: {:?}", response.stop_reason);

            Ok(())
        })
        .await?;

    Ok(())
}
