//! Direct, in-process access to the tools: no HTTP, no MCP.
//!
//! An in-game agent (companion, boss director, bot tester) runs its network calls on
//! its own thread and talks to the game through an [`AiLink`]. Requests execute on
//! the main thread at the end of a frame, so the game loop never waits for an LLM.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use bevy::prelude::*;
use serde_json::Value;

use crate::tools::{call_tool, CallOutcome};

struct AiRequest {
    agent: String,
    tool: String,
    args: Value,
    reply: Sender<CallOutcome>,
}

/// Cloneable, thread-safe handle for calling tools from any thread.
#[derive(Clone)]
pub struct AiLink {
    tx: Sender<AiRequest>,
}

impl AiLink {
    /// Queues a call; the receiver yields the outcome after the next frame ends.
    pub fn call(&self, agent: &str, tool: &str, args: Value) -> Receiver<CallOutcome> {
        let (reply, rx) = mpsc::channel();
        let _ = self.tx.send(AiRequest { agent: agent.to_owned(), tool: tool.to_owned(), args, reply });
        rx
    }

    /// Queues a call and waits for it. Never call this from the main (ECS) thread.
    pub fn call_blocking(&self, agent: &str, tool: &str, args: Value, timeout: Duration) -> Result<CallOutcome, String> {
        self.call(agent, tool, args).recv_timeout(timeout).map_err(|e| format!("no reply from the game: {e}"))
    }
}

/// Owns the receiving end; get a sender with [`AiLinkInbox::link`].
#[derive(Resource)]
pub struct AiLinkInbox {
    rx: Mutex<Receiver<AiRequest>>,
    link: AiLink,
}

impl Default for AiLinkInbox {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { rx: Mutex::new(rx), link: AiLink { tx } }
    }
}

impl AiLinkInbox {
    pub fn link(&self) -> AiLink {
        self.link.clone()
    }
}

const MAX_REQUESTS_PER_FRAME: usize = 64;

pub(crate) fn process_link_requests(world: &mut World) {
    let requests: Vec<AiRequest> = {
        let inbox = world.resource::<AiLinkInbox>();
        let Ok(rx) = inbox.rx.lock() else {
            return;
        };
        rx.try_iter().take(MAX_REQUESTS_PER_FRAME).collect()
    };
    for req in requests {
        let outcome = call_tool(world, &req.agent, &req.tool, req.args);
        let _ = req.reply.send(outcome);
    }
}
