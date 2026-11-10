//! Public Actr authority bootstrap for the Loran service composition.

use actr_protocol::realm_management::REALM_MANAGEMENT_PROTOCOL;
use actr_protocol::realm_profile::{IceEndpoint, REALM_PROFILE_PROTOCOL, SignalingEndpoint};
use actr_protocol::{
    AUTHORITY_DESCRIPTOR_VERSION, AuthorityDescriptor, ICE_CONFIG_PROTOCOL, IceConfiguration,
    IceServer, ServiceLink, parse_authority_member,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use issuer::realm_profile::{ProviderEndpoints, RealmProfiles};
use issuer::signer_client_wrapper::create_signer_client;
use loran_packages::PackageDistributor;
use platform::config::LoranConfig;
use platform::realm::{Realm, RealmProfileRecord, RealmStatus};
use std::{path::PathBuf, sync::Arc};
use url::Url;

struct AuthorityState {
    descriptor: AuthorityDescriptor,
    ice: IceConfiguration,
    /// Present when this Loran is the Actr Facility Provider for its own domain.
    profiles: Option<Arc<RealmProfiles>>,
}

/// Advertised HTTP origin and signaling URL of this instance.
fn advertised(config: &LoranConfig) -> anyhow::Result<(String, String)> {
    let http = config
        .bind
        .http
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("bind.http is required"))?;
    let port = http.effective_advertised_port();
    let default_port = (http.is_tls() && port == 443) || (!http.is_tls() && port == 80);
    let authority = if default_port {
        http.domain_name.clone()
    } else {
        format!("{}:{port}", http.domain_name)
    };
    let origin = format!("{}://{authority}", http.scheme());
    Url::parse(&origin).map_err(|error| anyhow::anyhow!("invalid advertised origin: {error}"))?;
    let signaling = format!("{}://{authority}/signaling/ws", http.ws_scheme());
    Ok((origin, signaling))
}

fn ice_servers(config: &LoranConfig) -> Vec<IceServer> {
    let mut ice_servers = Vec::new();
    if config.is_ice_enabled() {
        let ice = &config.bind.ice;
        let address = if ice.advertised_ip.contains(':') {
            format!("[{}]", ice.advertised_ip)
        } else {
            ice.advertised_ip.clone()
        };
        if config.is_stun_enabled() {
            ice_servers.push(IceServer {
                urls: vec![format!("stun:{address}:{}", ice.advertised_port)],
                username: None,
                credential: None,
            });
        }
        if config.is_turn_enabled() {
            ice_servers.push(IceServer {
                urls: vec![format!("turn:{address}:{}", ice.advertised_port)],
                username: None,
                credential: None,
            });
        }
    }
    ice_servers
}

/// Realm Profiles of this Loran as Actr Facility Provider for its own domain: the
/// authority is enabled and the Issuer has a Signer.
pub async fn build_realm_profiles(
    config: &LoranConfig,
) -> anyhow::Result<Option<Arc<RealmProfiles>>> {
    if !config.authority.enabled || !config.is_issuer_enabled() {
        return Ok(None);
    }
    let Some(signer_config) = config
        .services
        .issuer
        .as_ref()
        .and_then(|issuer| issuer.get_signer_client_config(config))
    else {
        return Ok(None);
    };
    let provider_id = config
        .authority
        .provider_id()
        .map_err(|error| anyhow::anyhow!(error))?;
    let (origin, signaling_href) = advertised(config)?;

    let mut signaling = Vec::new();
    if config.is_signaling_enabled() {
        signaling.push(SignalingEndpoint {
            url: signaling_href,
            priority: 0,
            control_plane_id: format!("{provider_id}/v1"),
            ticket_audience: "actr.signaling".to_string(),
        });
    }
    let ice = ice_servers(config)
        .into_iter()
        .map(|server| IceEndpoint {
            credential_audience: server
                .urls
                .iter()
                .any(|url| url.starts_with("turn"))
                .then(|| "actr.turn".to_string()),
            urls: server.urls,
        })
        .collect();
    let signer = create_signer_client(&signer_config, &config.loran_shared_key)
        .await
        .map_err(|error| anyhow::anyhow!("Signer client: {error}"))?;
    Ok(Some(Arc::new(RealmProfiles::new(
        ProviderEndpoints {
            provider_id,
            issuer_url: format!("{origin}/issuer"),
            signaling,
            ice,
            realms_base: format!("{origin}/actr/realms/"),
        },
        signer,
        config
            .services
            .issuer
            .as_ref()
            .map_or(3600, |issuer| issuer.server.token_ttl_secs),
        config.sqlite_path.clone(),
    ))))
}

async fn authority_composition(
    config: &LoranConfig,
    profiles: Option<Arc<RealmProfiles>>,
) -> anyhow::Result<(Option<AuthorityState>, Option<PackageDistributor>)> {
    let http = config
        .bind
        .http
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("bind.http is required"))?;
    let (origin, signaling_href) = advertised(config)?;

    let packages = config
        .services
        .packages
        .as_ref()
        .map(|packages| {
            let endpoint = Url::parse(&format!("{origin}/actr/packages"))?;
            let artifacts = packages.artifacts.iter().map(PathBuf::from);
            PackageDistributor::new(endpoint, artifacts).map_err(anyhow::Error::from)
        })
        .transpose()?;

    if !config.authority.enabled {
        return Ok((None, packages));
    }
    if config.env == "prod" && !http.is_tls() {
        anyhow::bail!("production authority discovery requires TLS");
    }
    let authority = config.authority.uri.clone().ok_or_else(|| {
        anyhow::anyhow!("authority.uri is required when authority.enabled is true")
    })?;
    let parsed_authority = parse_authority_member(&authority)
        .map_err(|error| anyhow::anyhow!("authority.uri is not a valid authority URI: {error}"))?;
    // The authority names a domain only. Its discovery document is fetched on
    // the default HTTPS port; an explicit bind.http port only appears in the
    // advertised service endpoints.
    let expected_authority =
        actr_protocol::uri::ActrUri::from_domain(&http.domain_name).map_err(|error| {
            anyhow::anyhow!("bind.http does not form a valid Actr authority: {error}")
        })?;
    if expected_authority.authority_uri() != parsed_authority.authority_uri() {
        anyhow::bail!(
            "authority.uri '{}' does not match advertised HTTP domain '{}'",
            parsed_authority.authority_uri(),
            expected_authority.authority_uri()
        );
    }

    let mut services = Vec::new();
    if config.is_signaling_enabled() {
        services
            .push(ServiceLink::new("signaling", &signaling_href).with_protocol("actr-signaling/1"));
        services
            .push(ServiceLink::new("discovery", &signaling_href).with_protocol("actr-discovery/1"));
    }
    if config.is_issuer_enabled() {
        services.push(ServiceLink::new("identity", format!("{origin}/issuer")));
    }
    if profiles.is_some() {
        let domain = parsed_authority.domain();
        services.push(
            ServiceLink::new("realms", format!("{origin}/actr/realms/{domain}"))
                .with_protocol(REALM_PROFILE_PROTOCOL),
        );
        services.push(
            ServiceLink::new(
                "realm-management",
                format!("{origin}/actr/realm-management"),
            )
            .with_protocol(REALM_MANAGEMENT_PROTOCOL),
        );
    }
    if config.is_ice_enabled() {
        services.push(
            ServiceLink::new("ice-config", format!("{origin}/actr/ice-config"))
                .with_protocol(ICE_CONFIG_PROTOCOL),
        );
    }
    if services.is_empty() {
        anyhow::bail!("an enabled authority must advertise at least one service");
    }

    let links = &config.authority.links;
    let descriptor = AuthorityDescriptor {
        version: AUTHORITY_DESCRIPTOR_VERSION,
        authority: parsed_authority.authority_uri(),
        services,
        links: (!links.is_empty()).then(|| links.clone()),
        realm_signing_keys: None,
        publisher: None,
    };
    descriptor
        .validate_for(&parsed_authority)
        .map_err(|error| anyhow::anyhow!("invalid authority descriptor: {error}"))?;

    Ok((
        Some(AuthorityState {
            descriptor,
            ice: IceConfiguration {
                version: 1,
                ice_servers: ice_servers(config),
            },
            profiles,
        }),
        packages,
    ))
}

async fn get_descriptor(State(state): State<Arc<AuthorityState>>) -> Response {
    let mut descriptor = state.descriptor.clone();
    if let Some(profiles) = &state.profiles {
        // Only the background task talks to the Signer; a public request never does.
        match profiles.signing_keys() {
            Some(member) => descriptor.realm_signing_keys = Some(member.clone()),
            None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    (
        [
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        Json(descriptor),
    )
        .into_response()
}

async fn get_ice_config(State(state): State<Arc<AuthorityState>>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "private, max-age=60")],
        Json(state.ice.clone()),
    )
}

/// `GET /actr/realms/{domain}/{realm_id}`: the accepted Realm Profile of an
/// active or retired Realm, served as signed. Unknown and pending Realms are
/// `404`.
async fn get_realm_profile(Path((domain, realm_id)): Path<(String, String)>) -> Response {
    let name = format!("{realm_id}@{domain}");
    let found = match Realm::get(&name).await {
        Ok(Some(realm)) if realm.status != RealmStatus::Pending => {
            RealmProfileRecord::get(&name).await
        }
        Ok(_) => Ok(None),
        Err(error) => Err(error),
    };
    let cors = (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    match found {
        Ok(Some(record)) => (
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "public, max-age=300"),
                cors,
            ],
            record.document,
        )
            .into_response(),
        Ok(None) => ([cors], StatusCode::NOT_FOUND).into_response(),
        Err(error) => {
            platform::recording::warn!("Realm Profile {name}: {error}");
            ([cors], StatusCode::INTERNAL_SERVER_ERROR).into_response()
        }
    }
}

pub async fn build_authority_router(
    config: &LoranConfig,
    profiles: Option<Arc<RealmProfiles>>,
) -> anyhow::Result<Router> {
    let (state, packages) = authority_composition(config, profiles).await?;
    let mut router = Router::new();
    if let Some(packages) = packages {
        platform::recording::info!("Adding Actr package hosting routes");
        for href in packages.artifact_hrefs() {
            platform::recording::info!("Hosting Actr package {href}");
        }
        router = router.nest("/actr/packages", packages.router());
    }
    if let Some(state) = state {
        platform::recording::info!(
            "Adding Actr authority discovery routes for {}",
            state.descriptor.authority
        );
        let profiles = state.profiles.clone();
        let mut routes = Router::new()
            .route("/.well-known/actr", get(get_descriptor))
            .route("/actr/ice-config", get(get_ice_config));
        if profiles.is_some() {
            routes = routes.route("/actr/realms/{domain}/{realm_id}", get(get_realm_profile));
        }
        router = router.merge(routes.with_state(Arc::new(state)));
        if let Some(profiles) = profiles {
            router = router.merge(super::realm_management::router(
                profiles,
                config.realm.clone(),
            ));
        }
    }
    Ok(router)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, body::to_bytes, http::Request, http::StatusCode};
    use platform::config::{PackagesConfig, authority::AuthorityConfig};
    use tower::ServiceExt;

    fn test_config(artifact: &std::path::Path) -> LoranConfig {
        let mut config = LoranConfig::default();
        config.enable = platform::config::ENABLE_SIGNALING
            | platform::config::ENABLE_STUN
            | platform::config::ENABLE_TURN
            | platform::config::ENABLE_ISSUER;
        config.bind.http.as_mut().unwrap().port = 8081;
        config.bind.http.as_mut().unwrap().advertised_port = Some(8081);
        config.bind.ice.advertised_ip = "127.0.0.1".to_string();
        config.bind.ice.advertised_port = 13478;
        config.authority = AuthorityConfig {
            enabled: true,
            uri: Some("actr://localhost".to_string()),
            ..AuthorityConfig::default()
        };
        config.services.packages = Some(PackagesConfig {
            artifacts: vec![artifact.display().to_string()],
        });
        config
    }

    /// `test_config` acting as Actr Facility Provider: the Issuer has a Signer.
    fn provider_config(artifact: &std::path::Path) -> LoranConfig {
        let mut config = test_config(artifact);
        config.services.issuer = Some(platform::config::IssuerServiceConfig {
            server: Default::default(),
            dependencies: platform::config::issuer::IssuerDependencies {
                signer: Some(Default::default()),
            },
        });
        config
    }

    /// Request path of the single artifact hosted by `config`.
    async fn artifact_uri(config: &LoranConfig) -> String {
        let (_, packages) = authority_composition(config, None).await.unwrap();
        let packages = packages.unwrap();
        let href = packages.artifact_hrefs().next().unwrap();
        href[url::Position::BeforePath..].to_string()
    }

    #[tokio::test]
    async fn descriptor_does_not_advertise_package_hosting() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();
        let config = test_config(&artifact);

        let (state, packages) = authority_composition(&config, None).await.unwrap();
        let state = state.unwrap();
        let packages = packages.unwrap();
        assert_eq!(state.descriptor.authority, "actr://localhost");
        assert!(state.descriptor.service("packages").is_none());
        assert_eq!(
            state.descriptor.require_service("signaling").unwrap().href,
            "ws://localhost:8081/signaling/ws"
        );
        assert_eq!(state.ice.ice_servers.len(), 2);

        let href = packages.artifact_hrefs().next().unwrap();
        assert!(
            href.as_str()
                .starts_with("http://localhost:8081/actr/packages/artifact?sha256="),
            "{href}"
        );
    }

    #[tokio::test]
    async fn authority_uri_names_a_domain_without_port() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();

        for (uri, expected) in [
            ("actr://localhost:8081", "not a valid authority URI"),
            ("actr://localhost/", "not a valid authority URI"),
            ("actr://example.com", "does not match"),
        ] {
            let mut config = test_config(&artifact);
            config.authority.uri = Some(uri.to_string());
            let error = authority_composition(&config, None)
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains(expected), "{uri}: {error}");
        }
    }

    #[tokio::test]
    async fn package_hosting_can_run_without_local_authority_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();
        let mut config = test_config(&artifact);
        config.authority.enabled = false;
        config.authority.uri = None;
        let app = build_authority_router(&config, None).await.unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::get(artifact_uri(&config).await)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::get("/.well-known/actr")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rejects_plain_http_authority_in_production() {
        let config = LoranConfig {
            env: "prod".to_string(),
            authority: AuthorityConfig {
                enabled: true,
                uri: Some("actr://localhost".to_string()),
                ..AuthorityConfig::default()
            },
            ..LoranConfig::default()
        };
        let error = authority_composition(&config, None).await.err().unwrap();
        assert!(error.to_string().contains("requires TLS"));
    }

    #[tokio::test]
    async fn public_http_routes_form_a_complete_bootstrap_chain() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        let artifact_bytes = b"complete actor package";
        std::fs::write(&artifact, artifact_bytes).unwrap();
        let config = test_config(&artifact);
        let app = build_authority_router(&config, None).await.unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::get("/.well-known/actr")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let descriptor: AuthorityDescriptor = serde_json::from_slice(&body).unwrap();
        let uri = "actr://localhost/echo"
            .parse::<actr_protocol::uri::ActrUri>()
            .unwrap();
        descriptor.validate_for(&uri).unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::get(artifact_uri(&config).await)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(body.as_ref(), artifact_bytes);

        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/actr/packages/artifact?sha256={}", "0".repeat(64)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::get("/actr/ice-config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let ice: IceConfiguration = serde_json::from_slice(&body).unwrap();
        assert_eq!(ice.ice_servers[0].urls, ["stun:127.0.0.1:13478"]);
    }

    #[tokio::test]
    async fn actr_facility_provider_publishes_realm_relations_and_links() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();
        let mut config = provider_config(&artifact);
        config.authority.links = vec!["/i/".to_string()];

        let profiles = build_realm_profiles(&config).await.unwrap();
        assert!(profiles.is_some());
        let (state, _) = authority_composition(&config, profiles.clone())
            .await
            .unwrap();
        let state = state.unwrap();
        let realms = state.descriptor.require_service("realms").unwrap();
        assert_eq!(realms.href, "http://localhost:8081/actr/realms/localhost");
        assert_eq!(realms.protocol.as_deref(), Some("actr-realm-profile/1"));
        let management = state
            .descriptor
            .require_service("realm-management")
            .unwrap();
        assert_eq!(
            management.href,
            "http://localhost:8081/actr/realm-management"
        );
        assert_eq!(
            management.protocol.as_deref(),
            Some("actr-realm-management/1")
        );
        assert_eq!(state.descriptor.links, Some(vec!["/i/".to_string()]));
        assert_eq!(state.profiles.unwrap().provider_id(), "localhost");

        // Without a Signer this Loran is not an Actr Facility Provider.
        let (state, _) = authority_composition(&test_config(&artifact), None)
            .await
            .unwrap();
        let state = state.unwrap();
        assert!(state.descriptor.service("realms").is_none());
        assert!(state.descriptor.service("realm-management").is_none());
        assert!(state.profiles.is_none());
        assert!(
            build_realm_profiles(&test_config(&artifact))
                .await
                .unwrap()
                .is_none()
        );

        config.authority.links = vec!["/".to_string()];
        let error = authority_composition(&config, profiles)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Realm link prefix"), "{error}");
    }

    #[test]
    fn realm_signing_keys_member_is_self_signed_over_jcs() {
        use actr_protocol::realm_profile::jcs;
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use ed25519_dalek::{Signer as _, Verifier as _};
        use issuer::realm_profile::{realm_signing_key_id, unsigned_realm_signing_keys};

        let key = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let key_id = realm_signing_key_id("provider.example");
        assert_eq!(key_id, "realm-signing:provider.example");
        let (mut member, payload) =
            unsigned_realm_signing_keys(&key_id, key.verifying_key().as_bytes());
        member.signature.value = BASE64.encode(key.sign(payload.as_bytes()).to_bytes());

        let descriptor = AuthorityDescriptor {
            version: AUTHORITY_DESCRIPTOR_VERSION,
            authority: "actr://provider.example".to_string(),
            services: vec![ServiceLink::new(
                "identity",
                "https://provider.example/issuer",
            )],
            links: None,
            realm_signing_keys: Some(member.clone()),
            publisher: None,
        };
        let uri = "actr://provider.example".parse().unwrap();
        descriptor.validate_for(&uri).unwrap();

        // The signature covers the member as served, minus `signature`.
        let mut served = serde_json::to_value(&descriptor).unwrap()["realm_signing_keys"].clone();
        served.as_object_mut().unwrap().remove("signature");
        assert_eq!(jcs(&served), payload);
        let signature: [u8; 64] = BASE64
            .decode(&member.signature.value)
            .unwrap()
            .try_into()
            .unwrap();
        key.verifying_key()
            .verify(
                payload.as_bytes(),
                &ed25519_dalek::Signature::from_bytes(&signature),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn descriptor_allows_any_origin_and_head() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();
        let app = build_authority_router(&test_config(&artifact), None)
            .await
            .unwrap();

        for method in ["GET", "HEAD"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/.well-known/actr")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method}");
            let headers = response.headers();
            assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            assert_eq!(headers[header::CONTENT_TYPE], "application/json");
            assert!(headers.contains_key(header::CACHE_CONTROL));
            let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            assert_eq!(body.is_empty(), method == "HEAD", "{method}");
        }
    }

    #[tokio::test]
    async fn descriptor_is_unavailable_until_the_realm_signing_key_is() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("echo.actr");
        std::fs::write(&artifact, b"actor package").unwrap();
        let mut config = provider_config(&artifact);
        // Nothing listens here.
        config
            .services
            .issuer
            .as_mut()
            .unwrap()
            .dependencies
            .signer
            .as_mut()
            .unwrap()
            .endpoint = "http://127.0.0.1:1".to_string();
        let profiles = build_realm_profiles(&config).await.unwrap();
        let app = build_authority_router(&config, profiles).await.unwrap();

        let response = app
            .oneshot(
                Request::get("/.well-known/actr")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
