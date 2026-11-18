//! 셸 항목(PIDL) 아이콘 얻기(`icon::shell_item_icon`). Windows 전용.
#![cfg(windows)]

use facet_win::icon::shell_item_icon;

/// 내 PC(`::{20D04FE0-3AEA-1069-A2D8-08002B30309D}`)의 셸 항목 식별자.
const THIS_PC_ID: [u8; 22] = [
    0x14, 0x00, 0x1F, 0x50, 0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08, 0x00,
    0x2B, 0x30, 0x30, 0x9D, 0x00, 0x00,
];

#[test]
fn 셸_항목의_아이콘을_요청_크기로_얻는다() {
    let icon = shell_item_icon(&THIS_PC_ID, 16).expect("내 PC 아이콘");
    assert_eq!((icon.width, icon.height), (16, 16));
    assert_eq!(icon.rgba.len(), 16 * 16 * 4);
    assert!(icon.rgba.chunks(4).any(|pixel| pixel[3] > 0), "빈 그림");
}

#[test]
fn 틀린_식별자와_크기는_none() {
    assert_eq!(shell_item_icon(&[4, 0, 9, 9], 16), None);
    assert_eq!(shell_item_icon(&THIS_PC_ID, 0), None);
    assert_eq!(shell_item_icon(&THIS_PC_ID, 257), None);
}
