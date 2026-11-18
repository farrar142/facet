//! 사적인 파이프(`pipe_client::private`): 하나뿐인 인스턴스, 같은 이름 거부, 붙은 클라이언트와 주고받기.
#![cfg(windows)]

use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use facet_win::pipe_client::{self, ConnectError, private::PrivatePipe};

const ACCESS: u32 = 0x0012_0089 | 0x0002;

fn name(tag: &str) -> String {
    format!(r"\\.\pipe\facet-test-private-{tag}-{}", std::process::id())
}

#[test]
fn 붙은_클라이언트와_주고받는다() {
    let name = name("roundtrip");
    let pipe = PrivatePipe::create(&name).unwrap();
    assert!(pipe.accept(Duration::from_millis(20)).unwrap().is_none());
    let client = pipe_client::connect(&name, ACCESS, Duration::from_secs(5)).unwrap();
    let server = pipe.accept(Duration::from_secs(5)).unwrap().expect("연결");
    let (mut sr, mut sw, _sc) = server.split().unwrap();
    let (mut cr, mut cw, _cc) = client.split().unwrap();
    cw.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    sr.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"ping");
    sw.write_all(b"pong").unwrap();
    cr.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"pong");
    assert!(
        pipe.accept(Duration::from_millis(10)).is_err(),
        "연결은 하나만"
    );
}

#[test]
fn 같은_이름은_다시_만들지_못하고_두_번째_클라이언트는_붙지_못한다() {
    let name = name("single");
    let _pipe = PrivatePipe::create(&name).unwrap();
    let err = PrivatePipe::create(&name).err().expect("같은 이름 거부");
    assert_eq!(err.kind(), ErrorKind::AddrInUse);
    let _first = pipe_client::connect(&name, ACCESS, Duration::from_secs(5)).unwrap();
    let second = pipe_client::connect(&name, ACCESS, Duration::from_millis(200));
    assert!(
        matches!(second, Err(ConnectError::Busy)),
        "인스턴스는 하나뿐"
    );
}
