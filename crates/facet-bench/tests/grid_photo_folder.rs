//! #331: 그리드 측정용 사진 폴더 만들기: 사진이 디코딩되고, 다시 돌려도 같고, 사진이 아닌 항목이 있으면 멈춘다.

use facet_bench::grid_screen::{make_photo_folder, solid_jpeg};

#[test]
fn 만든_사진은_썸네일로_디코딩된다() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jpg");
    std::fs::write(&path, solid_jpeg(4000, 3000, 3)).unwrap();
    let thumb = facet_core::thumbnail::make_thumbnail(&path, 96).unwrap();
    assert!(thumb.width <= 96 && thumb.height <= 96 && thumb.width > 0);
    assert!(
        std::fs::metadata(&path).unwrap().len() < 1_000_000,
        "단순한 무늬라 작다"
    );
}

#[test]
fn 사진_폴더는_빠진_것만_채우고_낯선_항목이면_멈춘다() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("photos");
    assert_eq!(make_photo_folder(&folder, 20).unwrap(), 20);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 20);
    assert_eq!(make_photo_folder(&folder, 20).unwrap(), 0);
    std::fs::remove_file(folder.join("photo-00003.jpg")).unwrap();
    assert_eq!(make_photo_folder(&folder, 20).unwrap(), 1);
    std::fs::write(folder.join("other.txt"), b"x").unwrap();
    assert!(make_photo_folder(&folder, 20).is_err());
}
