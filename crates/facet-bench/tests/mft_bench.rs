//! MFT 파싱 벤치마크의 재료와 상한 판정.

use std::time::Duration;

use facet_bench::{
    MFT_FIXTURE, MFT_PARSE_LIMIT, MFT_PARSE_TARGET, MFT_RECORD_SIZE, Measurement, check_mft_limit,
    measure_mft, mft_image,
};

#[test]
fn 조각을_되풀이한_덩어리는_모두_정상_레코드로_읽힌다() {
    let records = MFT_FIXTURE.len() / MFT_RECORD_SIZE * 3 + 7;
    let image = mft_image(records);
    assert_eq!(image.len(), records * MFT_RECORD_SIZE);
    assert_eq!(&image[..MFT_FIXTURE.len()], MFT_FIXTURE);

    let m = measure_mft(&image, 2).unwrap();
    assert_eq!(m.times.len(), 2);
    // 조각 하나보다 항목이 많다(되풀이한 레코드도 항목이 된다).
    let one = measure_mft(MFT_FIXTURE, 1).unwrap().item_count;
    assert!(m.item_count > one * 2, "{} / {one}", m.item_count);
}

#[test]
fn 상한은_로컬_기준의_10배이고_넘으면_넘은_수치를_알린다() {
    assert_eq!(MFT_PARSE_LIMIT, MFT_PARSE_TARGET * 10);
    let m = Measurement {
        item_count: 1,
        times: vec![Duration::from_millis(30), Duration::from_millis(40)],
    };
    assert!(check_mft_limit(&m, 100, Duration::from_millis(40)).is_ok());
    let err = check_mft_limit(&m, 100, Duration::from_millis(39)).unwrap_err();
    assert!(
        err.contains("40ms") && err.contains("39ms") && err.contains("100"),
        "{err}"
    );
}
