use super::*;
use tempfile::TempDir;

#[test]
fn test_template_context() {
    let ctx = TemplateContext::new(
        "my-chat-service",
        "ws://localhost:8080",
        DEFAULT_PUBLISHER,
        "echo-service",
        false,
    );
    assert_eq!(ctx.project_name, "my-chat-service");
    assert_eq!(ctx.project_name_snake, "my_chat_service");
    assert_eq!(ctx.project_name_pascal, "MyChatService");
    assert_eq!(ctx.workload_name, "MyChatServiceWorkload");
    assert_eq!(ctx.signaling_url, "ws://localhost:8080");
    assert_eq!(ctx.issuer_endpoint_url, "http://localhost:8080/issuer");
    assert_eq!(ctx.actr_swift_version, DEFAULT_ACTR_SWIFT_VERSION);
    assert_eq!(ctx.actr_workload_version, DEFAULT_ACTR_WORKLOAD_VERSION);
}

#[test]
fn rust_app_template_uses_the_workspace_upstream_overrides() {
    let temp_dir = TempDir::new().unwrap();
    let template = ProjectTemplate::new(ProjectTemplateName::Echo, SupportedLanguage::Rust);
    let context = TemplateContext::new("demo", "ws://localhost:8080", "acme", "echo", false);
    template.generate(temp_dir.path(), &context).unwrap();

    let generated: toml::Value =
        toml::from_str(&std::fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap())
            .unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let workspace: toml::Value = toml::from_str(&std::fs::read_to_string(root).unwrap()).unwrap();
    assert!(generated.get("workspace").is_some());
    let overrides = &generated["patch"]["crates-io"];
    assert!(overrides.get("sqlx").is_none());
    let dependencies = &workspace["workspace"]["dependencies"];
    for (package, dependency) in [
        ("webrtc", "webrtc"),
        ("rtc", "rtc"),
        ("rtc-turn", "rtc"),
        ("rtc-rtp", "rtc-rtp"),
    ] {
        assert_eq!(overrides[package]["git"], dependencies[dependency]["git"]);
        assert_eq!(overrides[package]["rev"], dependencies[dependency]["rev"]);
    }
}

#[test]
fn test_rust_cargo_templates_use_cli_version() {
    fn render_cargo_toml(template_name: ProjectTemplateName, is_service: bool) -> String {
        let temp_dir = TempDir::new().unwrap();
        let template = ProjectTemplate::new(template_name, SupportedLanguage::Rust);
        let context = TemplateContext::new(
            "test-project",
            "ws://localhost:8080",
            DEFAULT_PUBLISHER,
            "echo-service",
            is_service,
        );

        template
            .generate(temp_dir.path(), &context)
            .expect("Failed to generate Rust template");

        std::fs::read_to_string(temp_dir.path().join("Cargo.toml"))
            .expect("Failed to read generated Cargo.toml")
    }

    let version = DEFAULT_ACTR_WORKLOAD_VERSION;
    let app_cargo = render_cargo_toml(ProjectTemplateName::Echo, false);
    for dependency in ["actr-config", "actr-framework", "actr-protocol"] {
        assert!(
            app_cargo.contains(&format!(r#"{dependency} = "{version}""#)),
            "{dependency} should use the CLI version"
        );
    }
    assert!(
        app_cargo.contains(&format!(
            r#"actr-hyper = {{ version = "{version}", features = ["wasm-engine"] }}"#
        )),
        "actr-hyper should use the CLI version"
    );

    for template_name in [
        ProjectTemplateName::Echo,
        ProjectTemplateName::Empty,
        ProjectTemplateName::DataStream,
    ] {
        let service_cargo = render_cargo_toml(template_name, true);
        for dependency in ["actr-framework", "actr-protocol"] {
            assert!(
                service_cargo.contains(&format!(r#"{dependency} = "{version}""#)),
                "{dependency} should use the CLI version in the {template_name} template"
            );
        }
    }
}

#[test]
fn test_rust_data_stream_uses_empty_entry_placeholder() {
    fn render_lib_rs(template_name: ProjectTemplateName) -> String {
        let temp_dir = TempDir::new().unwrap();
        let template = ProjectTemplate::new(template_name, SupportedLanguage::Rust);
        let context = TemplateContext::new(
            "test-project",
            "ws://localhost:8080",
            DEFAULT_PUBLISHER,
            template_name.to_service_name(),
            true,
        );

        template.generate(temp_dir.path(), &context).unwrap();
        std::fs::read_to_string(temp_dir.path().join("src/lib.rs")).unwrap()
    }

    let empty_lib = render_lib_rs(ProjectTemplateName::Empty);
    let data_stream_lib = render_lib_rs(ProjectTemplateName::DataStream);

    assert_eq!(data_stream_lib, empty_lib);
    assert!(data_stream_lib.starts_with("// ACTR: generated Rust workload entry placeholder\n"));
}

#[test]
fn test_project_template_new() {
    let template = ProjectTemplate::new(ProjectTemplateName::Echo, SupportedLanguage::Swift);
    assert_eq!(template.name, ProjectTemplateName::Echo);
}

#[test]
fn test_project_template_generation() {
    let temp_dir = TempDir::new().unwrap();
    let template = ProjectTemplate::new(ProjectTemplateName::Echo, SupportedLanguage::Swift);
    let context = TemplateContext::new(
        "test-app",
        "ws://localhost:8080",
        DEFAULT_PUBLISHER,
        "echo-service",
        false,
    );

    template
        .generate(temp_dir.path(), &context)
        .expect("Failed to generate");

    // Verify project.yml exists
    assert!(temp_dir.path().join("project.yml").exists());
    // Verify manifest.toml exists
    assert!(temp_dir.path().join("manifest.toml").exists());
    // Linked Swift workloads resolve dependencies from manifest.toml at runtime.
    assert!(!temp_dir.path().join("manifest.lock.toml").exists());
    // Verify .gitignore exists
    assert!(temp_dir.path().join(".gitignore").exists());
    // Note: proto files are no longer created during init, they will be pulled via actr deps install
    // Verify app directory exists
    assert!(
        temp_dir
            .path()
            .join("TestApp")
            .join("TestApp.swift")
            .exists()
    );
}

#[test]
fn test_project_template_load_files() {
    let template = ProjectTemplate::new(ProjectTemplateName::Echo, SupportedLanguage::Swift);
    let context = TemplateContext::new(
        "test-app",
        "ws://localhost:8080",
        DEFAULT_PUBLISHER,
        "echo-service",
        false,
    );
    let result = template
        .lang_template
        .load_files(ProjectTemplateName::Echo, &context);
    assert!(result.is_ok());
}
