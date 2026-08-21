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
use wwt_page::{Input, Page};

use crate::event::Job;

/// The sending half of the pump. The core holds one.
pub struct InputPump {
    tx: mpsc::UnboundedSender<(Arc<Page>, Input)>,
}

impl InputPump {
    /// Start the pump.
    ///
    /// One pump for every page, not one per page: keys typed either side of
    /// a tab switch must not overtake each other, and two channels would make
    /// their order a matter of which task woke first.
    ///
    /// Failures are reported as a `Job` rather than returned: by the time a
    /// keystroke fails, whoever typed it has typed three more. They go on
    /// the channel every other finished page operation goes on, so the loop
    /// has one thing to select on rather than two.
    pub fn spawn(jobs: mpsc::UnboundedSender<Job>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(Arc<Page>, Input)>();

        tokio::spawn(async move {
            while let Some((page, input)) = rx.recv().await {
                if let Err(error) = page.dispatch(&input).await {
                    let _ = jobs.send(Job::Noted(error.to_string()));
                }
            }
        });

        Self { tx }
    }

    /// Queue one input. Never blocks, never fails: a closed channel means
    /// the pump task is gone, which only happens on the way out.
    pub fn send(&self, page: Arc<Page>, input: Input) {
        let _ = self.tx.send((page, input));
    }
}
