//! Shared `actr://` parsing and normalization vectors from
//! `contracts/actr/test-vectors/uri.json`.

use actr_protocol::{ActrId, uri::ActrUri};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    valid: Vec<ValidCase>,
    invalid: Vec<InvalidCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidCase {
    name: String,
    input: String,
    normalized: String,
    domain: String,
    realm_id: Option<String>,
    serial: Option<String>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
    realm_name: Option<String>,
    actor_id: Option<String>,
    is_local: bool,
    #[serde(default)]
    local_checks: Vec<LocalCheck>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalCheck {
    installation_id: String,
    result: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidCase {
    name: String,
    input: String,
    code: String,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../contracts/actr/test-vectors/uri.json"
    ))
    .expect("valid URI test vectors")
}

#[test]
fn valid_vectors_match_normalization_and_public_fields() {
    for case in vectors().valid {
        let name = &case.name;
        let uri = case
            .input
            .parse::<ActrUri>()
            .unwrap_or_else(|error| panic!("{name}: {error}"));

        assert_eq!(uri.to_string(), case.normalized, "{name}");
        assert_eq!(uri.scheme(), "actr", "{name}");
        assert_eq!(uri.domain(), case.domain, "{name}");
        assert_eq!(uri.realm_id(), case.realm_id.as_deref(), "{name}");
        assert_eq!(uri.serial(), case.serial.as_deref(), "{name}");
        assert_eq!(uri.path(), case.path, "{name}");
        assert_eq!(uri.query(), case.query.as_deref(), "{name}");
        assert_eq!(uri.fragment(), case.fragment.as_deref(), "{name}");
        assert_eq!(uri.realm_name(), case.realm_name, "{name}");
        assert_eq!(uri.actor_id(), case.actor_id, "{name}");
        assert_eq!(uri.is_local(), case.is_local, "{name}");

        let reparsed = case
            .normalized
            .parse::<ActrUri>()
            .unwrap_or_else(|error| panic!("{name}: normalized URI rejected: {error}"));
        assert_eq!(reparsed, uri, "{name}: normalization is not idempotent");
        assert_eq!(reparsed.to_string(), case.normalized, "{name}");

        if let Some(serial) = &case.serial {
            let actor = ActrId::from_uri(&uri)
                .unwrap_or_else(|| panic!("{name}: valid Actor URI rejected by ActrId"));
            assert_eq!(actor.to_string_repr(), case.actor_id.unwrap(), "{name}");
            assert_eq!(actor.realm_id(), case.realm_id.unwrap(), "{name}");
            assert_eq!(actor.domain(), case.domain, "{name}");
            assert_eq!(
                actor.serial_number,
                u64::from_str_radix(serial, 16).unwrap(),
                "{name}"
            );
        } else {
            assert_eq!(ActrId::from_uri(&uri), None, "{name}");
        }

        for check in case.local_checks {
            let result = uri.check_local(&check.installation_id);
            if check.result == "accept" {
                result.unwrap_or_else(|error| panic!("{name}: local check rejected: {error}"));
            } else {
                assert_eq!(
                    result.unwrap_err().code(),
                    check.result,
                    "{name}: foreign installation"
                );
            }
        }
    }
}

#[test]
fn invalid_vectors_return_stable_error_codes() {
    for case in vectors().invalid {
        let error = case.input.parse::<ActrUri>().unwrap_err();
        assert_eq!(error.code(), case.code, "{}: {error}", case.name);
    }
}
