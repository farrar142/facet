//! 단일 실행: 첫 facet 프로세스가 주인이 되어 모든 창을 띄우고, 다시 실행하면 인자를 주인에게 넘기고 끝난다.
//!
//! Windows 호출(뮤텍스·파이프)은 `facet_win::single_instance`가 하고, 여기서는 정하기만 한다.
//!
//! - 주인 확인([`start`])은 이름 있는 뮤텍스 하나를 만드는 일이라 UI 스레드가 디스크나 파이프를 기다리지 않는다.
//! - 이미 주인이 있으면 이 실행의 인자(경로 목록)와 작업 폴더를 넘기고 끝난다. 끝내 넘기지 못하면(주인이 막 끝나는
//!   중 등) 한 번 더 주인이 되어 보고, 그래도 안 되면 주인 없이 스스로 창을 연다. 실행이 아무 창도 띄우지 못하고
//!   사라지는 것보다 프로세스가 하나 더 뜨는 편이 낫다.
//! - 수신기([`facet_win::single_instance::Owner::listen`])는 자기 스레드에서 받는다. 받은 인자의 해석(파일 시스템에
//!   묻는다)도 그 스레드에서 하고, UI 스레드에는 새 창 열기만 맡긴다.

use std::path::PathBuf;

use facet_win::single_instance::{
    self, Claim, DEFAULT_FORWARD_TIMEOUT, InstanceName, Launch, Owner,
};

/// 이 프로세스가 어떻게 시작하는가.
pub enum Start {
    /// 창을 띄운다. 주인이면 주인 자리를 갖는다(주인 확인이 실패하면 주인 없이 띄운다).
    Run(Option<Owner>),
    /// 떠 있는 facet에 인자를 넘겼다. 창 없이 끝난다(종료 코드 0).
    Forwarded,
}

/// 이 실행의 인자(경로 목록)와 작업 폴더.
pub fn this_launch() -> Launch {
    Launch {
        working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        paths: std::env::args_os().skip(1).map(PathBuf::from).collect(),
    }
}

/// 주인이 되거나, 이미 주인이 있으면 `launch`를 넘긴다.
pub fn start(launch: &Launch) -> Start {
    let name = InstanceName::facet();
    match single_instance::claim(&name) {
        Ok(Claim::Owner(owner)) => return Start::Run(Some(owner)),
        Ok(Claim::Taken) => {}
        Err(error) => {
            eprintln!("단일 실행 주인을 확인하지 못함: {error}");
            return Start::Run(None);
        }
    }
    match single_instance::forward(&name, launch, DEFAULT_FORWARD_TIMEOUT) {
        Ok(()) => Start::Forwarded,
        Err(error) => {
            eprintln!("떠 있는 facet에 넘기지 못함: {error}");
            // 주인이 그 사이 끝났으면 이번에는 주인이 된다.
            match single_instance::claim(&name) {
                Ok(Claim::Owner(owner)) => Start::Run(Some(owner)),
                _ => Start::Run(None),
            }
        }
    }
}
