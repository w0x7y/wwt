//! Ordered input delivery.
//!
//! Every other page operation is idempotent or self-cancelling, so the core
//! spawns each one and lets them race. Keystrokes cannot: three keys as
//! three tasks would sometimes deliver `abc` as `acb`. One long-lived task
//! draining one channel makes ordering a property of the channel rather
//! than of the scheduler, and sending to an unbounded channel does not
//! await, so the loop still never blocks.

use std::sync::Arc;

use tokio::sync::mpsc;
use wwt_page::{KeyInput, MouseInput, Page};

use crate::session::Job;

/// One thing to send to the page.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Key(KeyInput),
    Mouse(MouseInput),
}

/// The sending half of the pump. The core holds one.
pub struct InputPump {
    tx: mpsc::UnboundedSender<Input>,
}

impl InputPump {
    /// Start the pump for a page.
    ///
    /// Failures are reported as a `Job` rather than returned: by the time a
    /// keystroke fails, whoever typed it has typed three more. They go on
    /// the channel every other finished page operation goes on, so the loop
    /// has one thing to select on rather than two.
    pub fn spawn(page: Arc<Page>, jobs: mpsc::UnboundedSender<Job>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Input>();

        tokio::spawn(async move {
            while let Some(input) = rx.recv().await {
                let result = match &input {
                    Input::Key(key) => page.dispatch_key(key).await,
                    Input::Mouse(mouse) => page.dispatch_mouse(mouse).await,
                };
                if let Err(error) = result {
                    let _ = jobs.send(Job::InputFailed(error.to_string()));
                }
            }
        });

        Self { tx }
    }

    /// Queue one input. Never blocks, never fails: a closed channel means
    /// the pump task is gone, which only happens on the way out.
    pub fn send(&self, input: Input) {
        let _ = self.tx.send(input);
    }
}
