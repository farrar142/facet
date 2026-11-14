//! `ui/app.slint`를 Rust 코드로 옮긴다.

fn main() {
    slint_build::compile("ui/app.slint").expect("ui/app.slint를 컴파일하지 못했다");
}
