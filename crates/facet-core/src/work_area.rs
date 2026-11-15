//! **작업 영역**의 상태: 창 아래쪽에 작업 대기열을 보이는 곳에 그릴 작업 목록.
//!
//! 작업 대기열([`crate::job::JobQueue`])은 사건만 보내고 지난 사건을 들고 있지 않는다. 화면이 작업마다 **진행
//! 표시**(종류, 대상, 항목 수, 바이트, 속도, 남은 시간, 지금 파일, 상태, 끝나면 결과)를 그리려면 마지막 진행·상태·
//! 결과를 들고 있어야 한다. 그것을 여기서 한다: 작업을 넣을 때 [`WorkArea::add`]로 더하고, 대기열의 사건을
//! [`WorkArea::apply`]로 반영한다. 끝난 작업은 사용자가 닫을 때까지([`WorkArea::dismiss`]) 결과와 함께 남는다.
//!
//! 앱 전체에 하나다([`crate::window::App::work_area`]): 모든 창에 같은 작업 영역이 보인다. 화면 문장은 없다
//! (ADR-0007).

use std::path::PathBuf;

use crate::job::{JobEvent, JobId, JobKind, JobState, Progress, Summary};

/// 작업 영역의 작업 하나: 진행 표시에 그릴 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEntry {
    /// 작업.
    pub id: JobId,
    /// 복사·이동.
    pub kind: JobKind,
    /// 대상 폴더.
    pub target: PathBuf,
    /// 넘긴 원본 항목 수(진행이 오기 전에 보인다).
    pub sources: usize,
    /// 지금 상태.
    pub state: JobState,
    /// 마지막 진행. 아직 오지 않았으면 `None`.
    pub progress: Option<Progress>,
    /// 결과 요약. 끝났으면 있다.
    pub summary: Option<Summary>,
}

impl JobEntry {
    /// 끝났는가(결과가 왔다). 끝난 작업만 닫을 수 있다.
    pub fn is_finished(&self) -> bool {
        self.summary.is_some()
    }
}

/// 작업 영역: 넣은 차례대로의 작업들.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkArea {
    entries: Vec<JobEntry>,
}

impl WorkArea {
    /// 빈 작업 영역.
    pub fn new() -> Self {
        WorkArea::default()
    }

    /// 넣은 작업을 더한다(대기 상태로). 이미 있는 작업이면 아무것도 하지 않는다.
    pub fn add(&mut self, id: JobId, kind: JobKind, target: PathBuf, sources: usize) {
        if self.entry(id).is_some() {
            return;
        }
        self.entries.push(JobEntry {
            id,
            kind,
            target,
            sources,
            state: JobState::Queued,
            progress: None,
            summary: None,
        });
    }

    /// 대기열의 사건 하나를 반영한다. 모르는 작업의 사건이면 아무것도 하지 않고 `false`다.
    ///
    /// 이름 충돌·항목별 오류 질문은 상태([`JobState::AwaitingAnswer`])로만 보인다(질문 대화는 화면 몫).
    pub fn apply(&mut self, event: &JobEvent) -> bool {
        let id = match event {
            JobEvent::Progress { job, .. }
            | JobEvent::StateChanged { job, .. }
            | JobEvent::Conflict { job, .. }
            | JobEvent::ItemError { job, .. }
            | JobEvent::Finished { job, .. } => *job,
        };
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        match event {
            JobEvent::Progress { progress, .. } => entry.progress = Some(progress.clone()),
            JobEvent::StateChanged { state, .. } => entry.state = *state,
            JobEvent::Conflict { .. } | JobEvent::ItemError { .. } => {
                entry.state = JobState::AwaitingAnswer;
            }
            JobEvent::Finished { summary, .. } => entry.summary = Some(summary.clone()),
        }
        true
    }

    /// 끝난 작업을 작업 영역에서 닫는다. 없거나 끝나지 않은 작업이면 `false`다.
    pub fn dismiss(&mut self, id: JobId) -> bool {
        match self.entries.iter().position(|e| e.id == id) {
            Some(at) if self.entries[at].is_finished() => {
                self.entries.remove(at);
                true
            }
            _ => false,
        }
    }

    /// 작업들을 넣은 차례대로.
    pub fn entries(&self) -> &[JobEntry] {
        &self.entries
    }

    /// 작업 `id`.
    pub fn entry(&self, id: JobId) -> Option<&JobEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// 보일 작업이 없는가. 그러면 작업 영역은 자리를 차지하지 않는다.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
