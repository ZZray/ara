//! Exact literal roster extracted from fixed OMP 596f2da descriptors.ts.
use super::descriptor_types::*;
use crate::model_collapse::VariantSpec;
use crate::model_identity_wire::{boolean, text};
use ara_rpc::{WireString, WireValue};
use std::sync::OnceLock;
fn strings(value: &VariantSpec, key: &str) -> Option<Vec<WireString>> {
    value
        .get(key)
        .and_then(WireValue::as_array)
        .map(|values| values.iter().filter_map(WireValue::as_string).cloned().collect())
}
// Bind the fixed upstream callbacks, keeping one identity per catalog entry.
fn factory_for(id: &WireString) -> Option<ModelManagerFactory> {
    use super::{google, ollama, openai_compat as compat, special};
    Some(match id.to_utf8().ok()?.as_str() {
        "abliteration" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::abliteration_model_manager_options(context, config))
        }),
        "aiand" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::aiand_model_manager_options(context, config, host))
        }),
        "aimlapi" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::aiml_api_model_manager_options(context, config))
        }),
        "alibaba-coding-plan" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::alibaba_coding_plan_model_manager_options(context, config))
        }),
        "alibaba-token-plan" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::alibaba_token_plan_model_manager_options(context, config))
        }),
        "baseten" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::baseten_model_manager_options(context, config))
        }),
        "bedrock-mantle" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::bedrock_mantle_model_manager_options(context, config))
        }),
        "anthropic" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::anthropic_model_manager_options(context, config, host))
        }),
        "cerebras" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::cerebras_model_manager_options(context, config))
        }),
        "cloudflare-ai-gateway" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::cloudflare_ai_gateway_model_manager_options(context, config))
        }),
        "cursor" => ModelManagerFactory::new(|context, config, _host| {
            Ok(special::cursor_model_manager_options(
                context,
                special::CursorModelManagerConfig {
                    api_key: config.api_key,
                    base_url: config.base_url,
                    client_version: None,
                },
            ))
        }),
        "deepinfra" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::deepinfra_model_manager_options(context, config))
        }),
        "deepseek" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::deepseek_model_manager_options(context, config))
        }),
        "devin" => {
            ModelManagerFactory::new(|context, config, _host| Ok(special::devin_model_manager_options(context, config)))
        }
        "cline-pass" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::cline_pass_model_manager_options(context, config))
        }),
        "firepass" => ModelManagerFactory::new(|_context, _config, _host| Ok(compat::firepass_model_manager_options())),
        "fireworks" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::fireworks_model_manager_options(context, config, host))
        }),
        "github-copilot" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::github_copilot_model_manager_options(context, config, host))
        }),
        "gitlab-duo-agent" => ModelManagerFactory::new(|context, config, host| {
            Ok(special::gitlab_duo_workflow_model_manager_options(
                context,
                special::GitLabDuoWorkflowModelManagerConfig { common: config, ..Default::default() },
                host.clone(),
            ))
        }),
        "gmi-cloud" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::gmi_cloud_model_manager_options(context, config))
        }),
        "google" => {
            ModelManagerFactory::new(|context, config, _host| Ok(google::google_model_manager_options(context, config)))
        }
        "google-vertex" => {
            ModelManagerFactory::new(|_context, _config, _host| Ok(google::google_vertex_model_manager_options()))
        }
        "groq" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::groq_model_manager_options(context, config)))
        }
        "huggingface" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::huggingface_model_manager_options(context, config))
        }),
        "kilo" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::kilo_model_manager_options(context, config)))
        }
        "kimi-code" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::kimi_code_model_manager_options(context, config))
        }),
        "litellm" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::litellm_model_manager_options(context, config, host))
        }),
        "lm-studio" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::lm_studio_model_manager_options(context, config, host))
        }),
        "mistral" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::mistral_model_manager_options(context, config))
        }),
        "meta" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::meta_model_manager_options(context, config)))
        }
        "moonshot" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::moonshot_model_manager_options(context, config, host))
        }),
        "nanogpt" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::nano_gpt_model_manager_options(context, config))
        }),
        "nvidia" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::nvidia_model_manager_options(context, config)))
        }
        "novita" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::novita_model_manager_options(context, config)))
        }
        "ollama" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::ollama_model_manager_options(context, config, host))
        }),
        "ollama-cloud" => ModelManagerFactory::new(|context, config, _host| {
            Ok(ollama::ollama_cloud_model_manager_options(context, config))
        }),
        "openai" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::openai_model_manager_options(context, config)))
        }
        "opencode-go" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::opencode_go_model_manager_options(context, config, host))
        }),
        "opencode-zen" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::opencode_zen_model_manager_options(context, config, host))
        }),
        "openrouter" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::openrouter_model_manager_options(context, config, host))
        }),
        "qianfan" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::qianfan_model_manager_options(context, config))
        }),
        "qwen-portal" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::qwen_portal_model_manager_options(context, config))
        }),
        "sakana" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::sakana_model_manager_options(context, config, host))
        }),
        "siliconflow" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::siliconflow_model_manager_options(context, config, host))
        }),
        "siliconflow-cn" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::siliconflow_cn_model_manager_options(context, config, host))
        }),
        "synthetic" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::synthetic_model_manager_options(context, config))
        }),
        "together" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::together_model_manager_options(context, config))
        }),
        "umans" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::umans_model_manager_options(context, config)))
        }
        "venice" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::venice_model_manager_options(context, config)))
        }
        "vercel-ai-gateway" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::vercel_ai_gateway_model_manager_options(context, config))
        }),
        "vllm" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::vllm_model_manager_options(context, config, host))
        }),
        "wafer-serverless" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::wafer_serverless_model_manager_options(context, config))
        }),
        "coreweave" => ModelManagerFactory::new(|context, config, host| {
            Ok(compat::coreweave_model_manager_options(context, config, host))
        }),
        "xai" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::xai_model_manager_options(context, config)))
        }
        "xai-oauth" => {
            ModelManagerFactory::new(|context, config, _host| compat::xai_oauth_model_manager_options(context, config))
        }
        "xiaomi" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::xiaomi_model_manager_options(
                context,
                compat::XiaomiModelManagerConfig { common: config, ..Default::default() },
            ))
        }),
        "xiaomi-token-plan-ams" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::xiaomi_model_manager_options(
                context,
                compat::XiaomiModelManagerConfig {
                    common: config,
                    provider_id: Some("xiaomi-token-plan-ams".into()),
                    token_plan_region: Some("ams".into()),
                },
            ))
        }),
        "xiaomi-token-plan-cn" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::xiaomi_model_manager_options(
                context,
                compat::XiaomiModelManagerConfig {
                    common: config,
                    provider_id: Some("xiaomi-token-plan-cn".into()),
                    token_plan_region: Some("cn".into()),
                },
            ))
        }),
        "xiaomi-token-plan-sgp" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::xiaomi_model_manager_options(
                context,
                compat::XiaomiModelManagerConfig {
                    common: config,
                    provider_id: Some("xiaomi-token-plan-sgp".into()),
                    token_plan_region: Some("sgp".into()),
                },
            ))
        }),
        "yolo-auto" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::yolo_auto_model_manager_options(context, config))
        }),
        "zai" => ModelManagerFactory::new(|_context, _config, _host| Ok(special::zai_model_manager_options())),
        "zenmux" => {
            ModelManagerFactory::new(|context, config, _host| Ok(compat::zenmux_model_manager_options(context, config)))
        }
        "zhipu-coding-plan" => ModelManagerFactory::new(|context, config, _host| {
            Ok(compat::zhipu_coding_plan_model_manager_options(context, config))
        }),
        _ => return None,
    })
}
pub fn catalog_providers() -> &'static [ProviderCatalogEntry] {
    static ROWS: OnceLock<Vec<ProviderCatalogEntry>> = OnceLock::new();
    ROWS.get_or_init(|| {
        WireValue::parse(include_str!("catalog_entries.json"))
            .expect("fixed descriptor literal")
            .as_array()
            .expect("roster array")
            .iter()
            .map(|value| {
                let value = VariantSpec::from_wire(value.clone());
                let discovery = value.record("catalogDiscovery").map(|d| CatalogDiscoveryConfig {
                    label: text(&d, "label").expect("descriptor label"),
                    env_vars: strings(&d, "envVars"),
                    oauth_provider: text(&d, "oauthProvider"),
                    allow_unauthenticated: boolean(&d, "allowUnauthenticated"),
                });
                let id = text(&value, "id").expect("descriptor id");
                let create_model_manager_options = factory_for(&id);
                ProviderCatalogEntry {
                    id,
                    default_model: text(&value, "defaultModel").expect("default model"),
                    env_vars: strings(&value, "envVars"),
                    has_factory: create_model_manager_options.is_some(),
                    create_model_manager_options,
                    allow_unauthenticated: boolean(&value, "allowUnauthenticated"),
                    dynamic_models_authoritative: boolean(&value, "dynamicModelsAuthoritative"),
                    catalog_discovery: discovery,
                    special_model_manager: boolean(&value, "specialModelManager"),
                }
            })
            .collect()
    })
}
pub fn provider_descriptors() -> Vec<ProviderDescriptor> {
    catalog_providers()
        .iter()
        .filter(|d| d.create_model_manager_options.is_some() && !d.special_model_manager.unwrap_or(false))
        .map(|d| {
            let mut discovery = d.catalog_discovery.clone();
            if let Some(discovery) = &mut discovery
                && discovery.env_vars.is_none()
            {
                discovery.env_vars = Some(d.env_vars.clone().unwrap_or_default());
            }
            ProviderDescriptor {
                provider_id: d.id.clone(),
                create_model_manager_options: d
                    .create_model_manager_options
                    .clone()
                    .expect("filtered callable factory"),
                default_model: d.default_model.clone(),
                allow_unauthenticated: d.allow_unauthenticated,
                dynamic_models_authoritative: d.dynamic_models_authoritative,
                catalog_discovery: discovery,
            }
        })
        .collect()
}
pub fn get_catalog_provider_entry(id: &WireString) -> Option<&'static ProviderCatalogEntry> {
    catalog_providers().iter().find(|d| d.id == *id)
}
pub fn default_model_per_provider() -> Vec<(WireString, WireString)> {
    catalog_providers().iter().map(|d| (d.id.clone(), d.default_model.clone())).collect()
}
