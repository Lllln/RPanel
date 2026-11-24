use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
};

use eframe::egui::{Context, ViewportId};
use rpanel::core::json::{self, Operation, Report};

struct Job {
    source: Arc<String>,
    operation: Operation,
    reply: SyncSender<Report>,
    context: Context,
    viewport: ViewportId,
}

#[derive(Clone)]
pub struct Worker(Option<SyncSender<Job>>);

pub enum SubmitError {
    Busy,
    Offline,
}

impl Default for Worker {
    fn default() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job>(8);
        let worker = thread::Builder::new()
            .name("rpanel-json".into())
            .spawn(move || {
                for job in receiver {
                    let report = json::process(&job.source, job.operation);
                    if job.reply.send(report).is_ok() {
                        job.context.request_repaint_of(job.viewport);
                    }
                }
            });
        if let Err(error) = worker {
            log::warn!("无法启动 JSON 工作线程：{error}");
            return Self(None);
        }
        Self(Some(sender))
    }
}

impl Worker {
    pub fn submit(
        &self,
        source: Arc<String>,
        operation: Operation,
        context: &Context,
    ) -> Result<Receiver<Report>, SubmitError> {
        let sender = self.0.as_ref().ok_or(SubmitError::Offline)?;
        let (reply, response) = mpsc::sync_channel(1);
        let job = Job {
            source,
            operation,
            reply,
            context: context.clone(),
            viewport: context.viewport_id(),
        };
        sender.try_send(job).map_err(|error| match error {
            TrySendError::Full(_) => SubmitError::Busy,
            TrySendError::Disconnected(_) => SubmitError::Offline,
        })?;
        Ok(response)
    }
}
