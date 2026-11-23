use std::time::Duration;

#[cfg(any(windows, test))]
use std::{
    sync::mpsc::{Receiver, TryRecvError},
    time::Instant,
};

use super::Trigger;
#[cfg(windows)]
use super::windows::selection::{SelectionTarget, SelectionWorker};

pub type SelectionResult = Result<String, &'static str>;
#[cfg(any(windows, test))]
pub const TIMEOUT_MESSAGE: &str = "读取选区超时，请手动粘贴。";

pub struct Selection {
    #[cfg(windows)]
    worker: Option<SelectionWorker>,
}

impl Selection {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        #[cfg(windows)]
        let worker = SelectionWorker::start(wake)
            .inspect_err(|error| log::warn!("无法启动选区读取线程：{error}"))
            .ok();
        #[cfg(target_os = "macos")]
        let _ = wake;

        Self {
            #[cfg(windows)]
            worker,
        }
    }

    pub fn begin(&self, trigger: Trigger) -> SelectionRequest {
        #[cfg(windows)]
        {
            let Some(source) = trigger.source else {
                return SelectionRequest::unavailable("预览已打开，可手动粘贴文本。");
            };
            let Some(target) = SelectionTarget::capture(source) else {
                log::debug!("跳过选区读取：无法确认右键目标的焦点窗口");
                return SelectionRequest::unavailable("无法确认来源窗口的焦点，请手动粘贴。");
            };
            let Some(worker) = &self.worker else {
                return SelectionRequest::unavailable("选区读取不可用，请手动粘贴。");
            };
            let deadline = Instant::now() + Duration::from_millis(900);
            match worker.request(target, deadline) {
                Ok(result) => SelectionRequest {
                    state: State::Pending { deadline, result },
                    target: Some(target),
                },
                Err(message) => SelectionRequest::unavailable(message),
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = trigger;
            SelectionRequest::unavailable("可从剪贴板手动粘贴文本。")
        }
    }
}

pub struct SelectionRequest {
    state: State,
    #[cfg(windows)]
    target: Option<SelectionTarget>,
}

enum State {
    Ready(SelectionResult),
    #[cfg(any(windows, test))]
    Pending {
        deadline: Instant,
        result: Receiver<SelectionResult>,
    },
}

impl SelectionRequest {
    fn unavailable(message: &'static str) -> Self {
        Self {
            state: State::Ready(Err(message)),
            #[cfg(windows)]
            target: None,
        }
    }

    pub fn poll(&mut self) -> Option<Duration> {
        #[cfg(any(windows, test))]
        if let State::Pending { deadline, result } = &self.state {
            let outcome = match result.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Disconnected) => Err("选区读取中断，请手动粘贴。"),
                Err(TryRecvError::Empty) => {
                    if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                        return Some(remaining);
                    }
                    log::debug!("选区读取超过界面等待期限");
                    Err(TIMEOUT_MESSAGE)
                }
            };
            self.state = State::Ready(outcome);
        }
        None
    }

    pub fn finish(self) -> SelectionResult {
        match self.state {
            #[cfg(windows)]
            State::Ready(Ok(_)) if self.target.is_some_and(|target| !target.is_current()) => {
                log::debug!("丢弃选区：点击面板前来源窗口或焦点已更换");
                Err("来源窗口或焦点已更换，请手动粘贴。")
            }
            State::Ready(outcome) => outcome,
            #[cfg(any(windows, test))]
            State::Pending { .. } => Err(TIMEOUT_MESSAGE),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn pending(deadline: Instant) -> (mpsc::SyncSender<SelectionResult>, SelectionRequest) {
        let (reply, result) = mpsc::sync_channel(1);
        let request = SelectionRequest {
            state: State::Pending { deadline, result },
            #[cfg(windows)]
            target: None,
        };
        (reply, request)
    }

    #[test]
    fn expired_request_stops_waiting_for_worker() {
        let (reply, mut request) = pending(Instant::now() - Duration::from_secs(1));
        assert_eq!(request.poll(), None);
        assert!(reply.send(Ok("过期选区".into())).is_err());
        assert_eq!(request.finish(), Err(TIMEOUT_MESSAGE));
    }

    #[test]
    fn superseded_request_cannot_supply_text_to_next_panel() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let (old_reply, old_request) = pending(deadline);
        drop(old_request);

        let (new_reply, mut new_request) = pending(deadline);
        assert!(old_reply.send(Ok("旧选区".into())).is_err());
        assert!(new_request.poll().is_some());
        new_reply.send(Ok("新选区".into())).unwrap();
        assert_eq!(new_request.poll(), None);
        assert_eq!(new_request.finish(), Ok("新选区".into()));
    }
}
