use trillium_http::Status;

#[test]
fn from_code_lossy_converts_recognized_codes_exactly() {
    for code in [100, 101, 103, 200, 204, 301, 404, 418, 451, 503, 511] {
        assert_eq!(
            Status::from_code_lossy(code),
            Status::try_from(code).unwrap()
        );
    }
}

#[test]
fn from_code_lossy_maps_unrecognized_codes_to_their_class() {
    for (code, expected) in [
        (102, Status::Continue),
        (199, Status::Continue),
        (299, Status::Ok),
        (399, Status::MultipleChoice),
        (499, Status::BadRequest),
        (520, Status::InternalServerError),
        (599, Status::InternalServerError),
    ] {
        assert_eq!(Status::from_code_lossy(code), expected, "{code}");
    }
}

#[test]
fn from_code_lossy_treats_out_of_range_codes_as_server_errors() {
    for code in [0, 42, 99, 600, 999, u16::MAX] {
        assert_eq!(
            Status::from_code_lossy(code),
            Status::InternalServerError,
            "{code}"
        );
    }
}
