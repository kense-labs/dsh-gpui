//! Provider marks: the local brand artwork on the model-service surfaces.
//!
//! The artwork is derived from @lobehub/icons v6.15.0 (MIT); see
//! `ui/assets/providers/LICENSE`. It is vendored so the WASM UI never depends on
//! a remote icon host.
//!
//! These are the PNG renders, not the SVG sources: GPUI paints an `svg()`
//! element as a single-channel alpha mask tinted by `text_color`, which would
//! flatten "colored tile + white glyph" artwork into one silhouette. The PNGs
//! keep the source colors through `img()`. Regenerate them with
//! `node scripts/build-provider-marks.mjs` after editing the SVG sources.
//!
//! The mapping lives here, outside the wasm-only `web` module, so the host test
//! suite can exercise it.

// On the host the consumers live in the wasm-only `web` module, so the artwork
// constants are reachable only from the tests below.
#![cfg_attr(not(target_family = "wasm"), allow(dead_code))]

/// One provider's mark, or `None` when the provider has no known brand.
pub type ProviderLogo = Option<&'static [u8]>;

pub const DEEPSEEK: &[u8] = include_bytes!("../assets/providers/deepseek.png");
pub const SILICONFLOW: &[u8] = include_bytes!("../assets/providers/siliconflow.png");
pub const OPENROUTER: &[u8] = include_bytes!("../assets/providers/openrouter.png");
pub const OPENAI: &[u8] = include_bytes!("../assets/providers/openai.png");
pub const OLLAMA: &[u8] = include_bytes!("../assets/providers/ollama.png");
pub const ANTHROPIC: &[u8] = include_bytes!("../assets/providers/anthropic.png");
pub const GEMINI: &[u8] = include_bytes!("../assets/providers/gemini.png");
pub const MOONSHOT: &[u8] = include_bytes!("../assets/providers/moonshot.png");
pub const QWEN: &[u8] = include_bytes!("../assets/providers/qwen.png");
pub const ZHIPU: &[u8] = include_bytes!("../assets/providers/zhipu.png");
pub const MINIMAX: &[u8] = include_bytes!("../assets/providers/minimax.png");
pub const MISTRAL: &[u8] = include_bytes!("../assets/providers/mistral.png");
pub const GROQ: &[u8] = include_bytes!("../assets/providers/groq.png");
pub const XAI: &[u8] = include_bytes!("../assets/providers/xai.png");
pub const AZURE: &[u8] = include_bytes!("../assets/providers/azure.png");
pub const BEDROCK: &[u8] = include_bytes!("../assets/providers/bedrock.png");
pub const NVIDIA: &[u8] = include_bytes!("../assets/providers/nvidia.png");
pub const HUGGINGFACE: &[u8] = include_bytes!("../assets/providers/huggingface.png");
pub const CLOUDFLARE: &[u8] = include_bytes!("../assets/providers/cloudflare.png");

/// Match one provider mark against a single lowercase haystack.
///
/// Specific brands deliberately precede generic ones: "Azure OpenAI" and
/// "Amazon Bedrock" must not fall into the OpenAI or generic branches.
fn mark_in(text: &str) -> ProviderLogo {
    let contains = |needle: &str| text.contains(needle);
    if contains("硅基流动") || contains("siliconflow") || contains("silicon cloud") {
        Some(SILICONFLOW)
    } else if contains("深度求索") || contains("deepseek") {
        Some(DEEPSEEK)
    } else if contains("openrouter") {
        Some(OPENROUTER)
    } else if contains("ollama") {
        Some(OLLAMA)
    } else if contains("anthropic") || contains("claude") {
        Some(ANTHROPIC)
    } else if contains("gemini") || contains("google") {
        Some(GEMINI)
    } else if contains("moonshot") || contains("kimi") {
        Some(MOONSHOT)
    } else if contains("qwen") || contains("通义") {
        Some(QWEN)
    } else if contains("zhipu") || contains("glm") || contains("智谱") || contains("z.ai") {
        Some(ZHIPU)
    } else if contains("minimax") || contains("hailuo") || contains("海螺") {
        Some(MINIMAX)
    } else if contains("mistral") {
        Some(MISTRAL)
    } else if contains("groq") {
        Some(GROQ)
    } else if contains("xai") || contains("grok") {
        Some(XAI)
    } else if contains("azure") {
        Some(AZURE)
    } else if contains("bedrock") || contains("amazon") {
        Some(BEDROCK)
    } else if contains("nvidia") {
        Some(NVIDIA)
    } else if contains("huggingface") || contains("hugging face") {
        Some(HUGGINGFACE)
    } else if contains("cloudflare") {
        Some(CLOUDFLARE)
    } else if contains("openai") {
        // Generic last: several vendors expose an OpenAI-compatible path.
        Some(OPENAI)
    } else {
        None
    }
}

/// Resolve the mark for a configured provider.
///
/// The name is authoritative — a provider is identified by what the user called
/// it — and the base URL is only consulted when the name matches nothing.
/// Testing the concatenation let a shared path segment hijack the branch, e.g.
/// Groq's real endpoint `https://api.groq.com/openai/v1` matched OpenAI.
pub fn provider_logo(name: &str, base_url: &str) -> ProviderLogo {
    mark_in(&name.to_lowercase()).or_else(|| mark_in(&base_url.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(name: &str, url: &str) -> ProviderLogo {
        provider_logo(name, url)
    }

    #[test]
    fn provider_names_map_to_their_own_marks() {
        assert_eq!(mark("DeepSeek", "https://api.deepseek.com"), Some(DEEPSEEK));
        assert_eq!(
            mark("硅基流动", "https://api.siliconflow.cn/v1"),
            Some(SILICONFLOW)
        );
        assert_eq!(
            mark("OpenRouter", "https://openrouter.ai/api/v1"),
            Some(OPENROUTER)
        );
        assert_eq!(mark("OpenAI", "https://api.openai.com/v1"), Some(OPENAI));
        assert_eq!(mark("Ollama", "http://localhost:11434/v1"), Some(OLLAMA));
        assert_eq!(
            mark("Moonshot AI", "https://api.moonshot.cn/v1"),
            Some(MOONSHOT)
        );
        assert_eq!(mark("Kimi", "https://api.moonshot.cn/v1"), Some(MOONSHOT));
        assert_eq!(
            mark("通义千问", "https://dashscope.aliyuncs.com"),
            Some(QWEN)
        );
        assert_eq!(
            mark("智谱AI", "https://open.bigmodel.cn/api/paas/v4"),
            Some(ZHIPU)
        );
        assert_eq!(mark("xAI", "https://api.x.ai/v1"), Some(XAI));
        assert_eq!(
            mark("NVIDIA", "https://integrate.api.nvidia.com/v1"),
            Some(NVIDIA)
        );
        assert_eq!(
            mark("Cloudflare", "https://api.cloudflare.com/client/v4"),
            Some(CLOUDFLARE)
        );
    }

    #[test]
    fn a_shared_openai_compatible_path_does_not_hijack_the_brand() {
        // Groq's real endpoint ends in `/openai/v1`; the name must win.
        assert_eq!(mark("Groq", "https://api.groq.com/openai/v1"), Some(GROQ));
        // Azure exposes OpenAI models but keeps its own mark.
        assert_eq!(
            mark("Azure OpenAI", "https://x.openai.azure.com"),
            Some(AZURE)
        );
        assert_eq!(
            mark(
                "Amazon Bedrock",
                "https://bedrock-runtime.us-east-1.amazonaws.com"
            ),
            Some(BEDROCK)
        );
    }

    #[test]
    fn the_base_url_is_the_fallback_when_the_name_is_unrecognized() {
        assert_eq!(
            mark("我的自建服务", "https://api.deepseek.com/v1"),
            Some(DEEPSEEK)
        );
        assert_eq!(mark("自定义", "https://api.anthropic.com"), Some(ANTHROPIC));
        // Unknown on both counts: no mark, and the caller falls back to a glyph.
        assert_eq!(mark("我的自建服务", "https://my-llm.internal/v1"), None);
    }

    #[test]
    fn every_mark_is_a_decodable_png() {
        const MARKS: [(&str, &[u8]); 19] = [
            ("anthropic", ANTHROPIC),
            ("azure", AZURE),
            ("bedrock", BEDROCK),
            ("cloudflare", CLOUDFLARE),
            ("deepseek", DEEPSEEK),
            ("gemini", GEMINI),
            ("groq", GROQ),
            ("huggingface", HUGGINGFACE),
            ("minimax", MINIMAX),
            ("mistral", MISTRAL),
            ("moonshot", MOONSHOT),
            ("nvidia", NVIDIA),
            ("ollama", OLLAMA),
            ("openai", OPENAI),
            ("openrouter", OPENROUTER),
            ("qwen", QWEN),
            ("siliconflow", SILICONFLOW),
            ("xai", XAI),
            ("zhipu", ZHIPU),
        ];
        for (name, bytes) in MARKS {
            assert!(
                bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
                "{name} is not a PNG"
            );
            assert!(bytes.len() > 100, "{name} looks truncated");
        }
    }
}
