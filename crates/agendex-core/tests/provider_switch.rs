use agendex_core::agent::{AgendexAgent, AgentConfig};
use agendex_core::model::ModelFormat;
use std::sync::Mutex;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn test_provider_slash_command_flow() -> anyhow::Result<()> {
    let _lock = ENV_MUTEX.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("agendex_provider_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let test_cfg_path = tmp.join("config.toml");
    std::env::set_var("AGENDEX_CONFIG", &test_cfg_path);

    let config = AgentConfig {
        workspace_dir: tmp.clone(),
        ..Default::default()
    };

    let agent = AgendexAgent::init(config).await?;

    // 1. /provider (or /providers) lists available presets and active provider
    let list_out = agent.process_turn("/provider").await?;
    assert!(
        list_out.contains("Active LLM Provider:"),
        "Expected provider header in: {}",
        list_out
    );
    assert!(
        list_out.contains("strata"),
        "Expected strata in provider list"
    );
    assert!(list_out.contains("nim"), "Expected nim in provider list");

    // 2. /provider switch strata
    let switch_strata = agent.process_turn("/provider switch strata").await?;
    assert!(
        switch_strata.contains("Switched active provider to 'strata'"),
        "Unexpected: {}",
        switch_strata
    );

    {
        let active_name = agent.active_provider_name.read().await.clone();
        assert_eq!(active_name, "strata");

        let active_model = agent.active_model_config.read().await.clone();
        assert_eq!(active_model.format, ModelFormat::OpenAi);
        assert_eq!(active_model.model_name, "huihui-swift-1.5");
        assert_eq!(
            active_model.base_url.as_deref(),
            Some("http://127.0.0.1:8080/v1")
        );
        assert_eq!(active_model.api_key, "strata");
    }

    // 3. /provider nim (shortcut syntax)
    let switch_nim = agent
        .process_turn("/provider nim mistralai/mistral-large-2-instruct")
        .await?;
    assert!(
        switch_nim.contains("Switched active provider to 'nim'"),
        "Unexpected: {}",
        switch_nim
    );

    {
        let active_name = agent.active_provider_name.read().await.clone();
        assert_eq!(active_name, "nim");

        let active_model = agent.active_model_config.read().await.clone();
        assert_eq!(
            active_model.model_name,
            "mistralai/mistral-large-2-instruct"
        );
        assert_eq!(
            active_model.base_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
    }

    // 4. /provider model
    let model_change = agent
        .process_turn("/provider model meta/llama-3.1-70b-instruct")
        .await?;
    assert!(
        model_change.contains("Updated model for provider 'nim' to 'meta/llama-3.1-70b-instruct'"),
        "Unexpected: {}",
        model_change
    );

    {
        let active_model = agent.active_model_config.read().await.clone();
        assert_eq!(active_model.model_name, "meta/llama-3.1-70b-instruct");
    }

    // 5. /provider add custom
    let add_out = agent
        .process_turn(
            "/provider add custom-vllm http://127.0.0.1:8000/v1 custom-llm openai sk-test-key",
        )
        .await?;
    assert!(
        add_out.contains("Added provider 'custom-vllm'"),
        "Unexpected: {}",
        add_out
    );

    let switch_custom = agent.process_turn("/provider switch custom-vllm").await?;
    assert!(
        switch_custom.contains("Switched active provider to 'custom-vllm'"),
        "Unexpected: {}",
        switch_custom
    );

    {
        let active_name = agent.active_provider_name.read().await.clone();
        assert_eq!(active_name, "custom-vllm");

        let active_model = agent.active_model_config.read().await.clone();
        assert_eq!(active_model.model_name, "custom-llm");
        assert_eq!(
            active_model.base_url.as_deref(),
            Some("http://127.0.0.1:8000/v1")
        );
        assert_eq!(active_model.api_key, "sk-test-key");
    }

    std::env::remove_var("AGENDEX_CONFIG");
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn test_provider_add_4_part_persists_to_env_and_uses_ref() -> anyhow::Result<()> {
    let _lock = ENV_MUTEX.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("agendex_prov_4part_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let test_cfg_path = tmp.join(".agendex").join("config.toml");
    std::env::set_var("AGENDEX_HOME", &tmp);
    std::env::set_var("AGENDEX_CONFIG", &test_cfg_path);

    let config = AgentConfig {
        workspace_dir: tmp.clone(),
        ..Default::default()
    };

    let agent = AgendexAgent::init(config).await?;

    // 4-part form: /provider add nim nvapi-secret-nim-token-999
    let add_out = agent
        .process_turn("/provider add nim nvapi-secret-nim-token-999")
        .await?;
    assert!(add_out.contains("Added provider 'nim'"));

    // Check that ~/.agendex/.env was written with the secret key
    let env_file = tmp.join(".agendex").join(".env");
    assert!(env_file.exists(), ".env file must exist in AGENDEX_HOME");
    let env_content = std::fs::read_to_string(&env_file)?;
    assert!(env_content.contains("NIM_API_KEY=nvapi-secret-nim-token-999"));

    // Check that config.toml does NOT contain the secret key, only the $NIM_API_KEY reference
    let cfg_content = std::fs::read_to_string(&test_cfg_path)?;
    assert!(
        !cfg_content.contains("nvapi-secret-nim-token-999"),
        "config.toml must never contain literal API key"
    );
    assert!(
        cfg_content.contains("$NIM_API_KEY"),
        "config.toml must contain $NIM_API_KEY reference"
    );

    // Switch to nim and verify active model key resolves correctly
    let _ = agent.process_turn("/provider switch nim").await?;
    {
        let active_model = agent.active_model_config.read().await.clone();
        assert_eq!(active_model.api_key, "nvapi-secret-nim-token-999");
    }

    std::env::remove_var("AGENDEX_HOME");
    std::env::remove_var("AGENDEX_CONFIG");
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn test_provider_add_key_before_model() -> anyhow::Result<()> {
    let _lock = ENV_MUTEX.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("agendex_prov_kbm_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let test_cfg_path = tmp.join(".agendex").join("config.toml");
    std::env::set_var("AGENDEX_HOME", &tmp);
    std::env::set_var("AGENDEX_CONFIG", &test_cfg_path);

    let config = AgentConfig {
        workspace_dir: tmp.clone(),
        ..Default::default()
    };

    let agent = AgendexAgent::init(config).await?;

    // 1. Key before model with explicit base URL
    // /provider add custom-kbm http://127.0.0.1:8000/v1 sk-FAKEseven custom-model
    let add_out1 = agent
        .process_turn("/provider add custom-kbm http://127.0.0.1:8000/v1 sk-FAKEseven custom-model")
        .await?;

    // Model must be custom-model, NOT [REDACTED]
    assert!(
        add_out1.contains("model: custom-model"),
        "Expected 'model: custom-model' in reply, got: {}",
        add_out1
    );
    assert!(
        !add_out1.contains("model: [REDACTED]"),
        "Must not mask the model field: {}",
        add_out1
    );
    assert!(
        !add_out1.contains("sk-FAKEseven"),
        "Must not leak raw key in reply: {}",
        add_out1
    );

    // Verify .env persisted the actual key
    let env_file = tmp.join(".agendex").join(".env");
    let env_content = std::fs::read_to_string(&env_file)?;
    assert!(
        env_content.contains("CUSTOM_KBM_API_KEY=sk-FAKEseven"),
        "Expected actual key in .env, got: {}",
        env_content
    );

    // 2. Preset with key before model: /provider add nim nvapi-FAKEfive meta/llama-3.1-70b-instruct
    let add_out2 = agent
        .process_turn("/provider add nim nvapi-FAKEfive meta/llama-3.1-70b-instruct")
        .await?;

    assert!(
        add_out2.contains("model: meta/llama-3.1-70b-instruct"),
        "Expected 'model: meta/llama-3.1-70b-instruct' in reply, got: {}",
        add_out2
    );
    assert!(
        !add_out2.contains("model: [REDACTED]"),
        "Must not mask the model field: {}",
        add_out2
    );
    assert!(
        !add_out2.contains("nvapi-FAKEfive"),
        "Must not leak raw key in reply: {}",
        add_out2
    );

    // 3. 5-argument syntax with unprefixed key (e.g. plainFAKEkey5arg):
    // /provider add custom-unprefixed http://127.0.0.1:8000/v1 plainFAKEkey5arg
    let add_out3 = agent
        .process_turn("/provider add custom-unprefixed http://127.0.0.1:8000/v1 plainFAKEkey5arg")
        .await?;

    assert!(
        !add_out3.contains("model: plainFAKEkey5arg"),
        "Must NOT treat unprefixed key as model name: {}",
        add_out3
    );
    assert!(
        !add_out3.contains("plainFAKEkey5arg"),
        "Must not echo plain text key in output: {}",
        add_out3
    );
    assert!(
        add_out3.contains("$CUSTOM_UNPREFIXED_API_KEY"),
        "Must reference environment variable for key: {}",
        add_out3
    );

    // Verify .env has the unprefixed key persisted
    let env_content3 = std::fs::read_to_string(&env_file)?;
    assert!(
        env_content3.contains("CUSTOM_UNPREFIXED_API_KEY=plainFAKEkey5arg"),
        "Expected plainFAKEkey5arg in .env: {}",
        env_content3
    );

    // Verify config.toml does NOT have plain text key under model or api_key
    let cfg_content = std::fs::read_to_string(&test_cfg_path)?;
    assert!(
        !cfg_content.contains("model = \"plainFAKEkey5arg\""),
        "config.toml must not store key as model: {}",
        cfg_content
    );
    assert!(
        !cfg_content.contains("api_key = \"plainFAKEkey5arg\""),
        "config.toml must not store plain text key: {}",
        cfg_content
    );
    assert!(
        cfg_content.contains("api_key = \"$CUSTOM_UNPREFIXED_API_KEY\""),
        "config.toml must store $CUSTOM_UNPREFIXED_API_KEY reference: {}",
        cfg_content
    );

    // 4. Persistence across process restart: remove env var from memory, load env, expand
    std::env::remove_var("CUSTOM_UNPREFIXED_API_KEY");
    agendex_core::config::load_agendex_env();
    let expanded = agendex_core::config::expand_env_var_or_literal("$CUSTOM_UNPREFIXED_API_KEY");
    assert_eq!(
        expanded, "plainFAKEkey5arg",
        "Key must persist and reload across restarts from .env"
    );

    std::env::remove_var("AGENDEX_HOME");
    std::env::remove_var("AGENDEX_CONFIG");
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
