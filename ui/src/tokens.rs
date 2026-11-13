//! DeepSeek Harness 官方设计 token。
//! 全部数值原样提取自官方 dsh-web-frontend 构建产物（dist CSS + client-ui-theme 注入样式），
//! 明/暗两套与官方 body / body[data-ds-dark-theme] 一一对应，勿凭感觉修改。

use gpui::{App, Hsla, rgb, rgba};
use kui::foundation::{Theme, ThemeMode};

/// 官方 token 集（按主题模式解析后的具体值）。
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    // 背景
    pub bg_base: Hsla,            // --dsw-alias-bg-base
    pub bg_layer_1: Hsla,         // --dsw-alias-bg-layer-1
    pub bg_layer_2: Hsla,         // --dsw-alias-bg-layer-2
    pub bg_layer_3: Hsla,         // --dsw-alias-bg-layer-3
    pub bg_mask_1: Hsla,          // --dsw-alias-bg-mask-1（模态遮罩）
    pub bg_module_platform: Hsla, // --dsw-alias-bg-module-platform（分段容器）
    pub bg_overlay: Hsla,         // --dsw-alias-bg-overlay
    // 特定场景
    pub sidebar_fill: Hsla,              // --dsw-specific-sidebar-fill
    pub sidebar_nav_active: Hsla,        // --dsw-specific-sidebar-nav-item-active
    pub sidebar_nav_active_accent: Hsla, // --dsw-specific-sidebar-nav-item-active-accent
    pub input_major: Hsla,               // --dsw-specific-input-major（composer 卡）
    pub login_input: Hsla,               // --dsw-specific-login-input
    pub bubble: Hsla,                    // --dsw-specific-bubble（用户气泡）
    pub selector: Hsla,                  // --dsw-specific-selector（＋按钮底）
    pub tip: Hsla,                       // --dsw-specific-tip
    // 边框（发丝线）
    pub border_l1: Hsla, // --dsw-alias-border-l1
    pub border_l2: Hsla, // --dsw-alias-border-l2
    pub border_l3: Hsla, // --dsw-alias-border-l3
    pub border_l4: Hsla, // --dsw-alias-border-l4
    // 文字
    pub label_primary: Hsla,            // --dsw-alias-label-primary
    pub label_secondary: Hsla,          // --dsw-alias-label-secondary
    pub label_tertiary: Hsla,           // --dsw-alias-label-tertiary
    pub label_caption: Hsla,            // --dsw-alias-label-caption
    pub label_dimmed: Hsla,             // --dsw-alias-label-dimmed
    pub label_primary_inverted: Hsla,   // --dsw-alias-label-primary-inverted（深底上的主文字）
    pub label_primary_foreground: Hsla, // --dsw-alias-label-primary-foreground（primary 按钮上的字）
    pub label_primary_bluish: Hsla,     // --dsw-alias-label-primary-bluish（徽章字）
    // 交互
    pub hover: Hsla,        // --dsw-alias-interactive-bg-hover
    pub hover_active: Hsla, // --dsw-alias-interactive-bg-active
    pub hover_accent: Hsla, // --dsw-alias-interactive-bg-hover-accent
    pub hover_danger: Hsla, // --dsw-alias-interactive-bg-hover-danger
    pub hover_solid: Hsla,  // --dsw-alias-interactive-bg-hover-solid
    // 按钮
    pub button_primary_fill: Hsla, // --dsw-alias-button-primary-fill（近白/近黑单色）
    pub button_primary_hover: Hsla, // --dsw-alias-button-primary-hover
    pub button_elevated: Hsla,     // --dsw-alias-button-elevated-fill
    pub button_floating: Hsla,     // --dsw-alias-button-floating-fill
    pub button_floating_hover: Hsla, // --dsw-alias-button-floating-hover
    pub button_ghost_active: Hsla, // --dsw-alias-button-ghost-active-fill
    pub button_info_fill: Hsla,    // --dsw-alias-button-info-fill（品牌蓝）
    pub button_info_hover: Hsla,   // --dsw-alias-button-info-hover
    // 状态
    pub state_business_primary: Hsla, // --dsw-alias-state-business-primary（品牌蓝）
    pub state_business_tertiary: Hsla, // --dsw-alias-state-business-tertiary（徽章底）
    pub state_success_primary: Hsla,  // --dsw-alias-state-success-primary
    pub state_success_secondary: Hsla, // --dsw-alias-state-success-secondary
    pub state_success_tertiary: Hsla, // --dsw-alias-state-success-tertiary
    pub state_error_primary: Hsla,    // --dsw-alias-state-error-primary
    pub state_error_secondary: Hsla,  // --dsw-alias-state-error-secondary
    pub state_warn_primary: Hsla,     // --dsw-alias-state-warn-primary
    pub state_warn_label: Hsla,       // --dsw-alias-state-warn-label
    pub state_warn_tertiary: Hsla,    // --dsw-alias-state-warn-tertiary
    pub state_idle_primary: Hsla,     // --dsw-alias-state-idle-primary
    // 链接 / 品牌
    pub link: Hsla,         // --dsw-alias-link
    pub brand_accent: Hsla, // --dsw-alias-brand-primary-new-colorprimary-new-color
    // 浮层
    pub menu_stroke: Hsla, // --dsw-elevation-stroke-color on [data-menu-material]
    pub menu_surface_fill: Hsla, // --dsw-menu-surface-fill（磨砂层的着色）
    pub menu_surface: Hsla, // --dsw-specific-menu（darwin 不透明底衬）
    pub menu_group_header: Hsla, // --dsw-alias-menu-group-header-fill
    pub toast_bg: Hsla,    // --dsw-alias-toast-bg
    /// --dsw-alias-toast-label：静态 bluish-00，明暗两套主题下都是纯白。
    pub toast_label: Hsla,
    pub tooltip_bg: Hsla, // --dsw-alias-tooltip-bg
    // 滚动条
    pub scrollbar_l1: Hsla,       // --dsw-alias-scrollbar-bg-l1
    pub scrollbar_l1_hover: Hsla, // hover-l1
    pub scrollbar_l2: Hsla,       // --dsw-alias-scrollbar-bg-l2（菜单/侧栏）
    pub scrollbar_l2_hover: Hsla, // hover-l2
    // Markdown / 代码
    pub md_code_block: Hsla,        // --dsw-alias-markdown-code-block
    pub md_code_block_banner: Hsla, // --dsw-alias-markdown-code-block-banner
    pub md_inline_code: Hsla,       // --dsw-alias-markdown-inline-code
    pub md_tag: Hsla,               // --dsw-alias-markdown-tag
    // 开关
    pub switch_thumb: Hsla, // --dsw-alias-switch-thumb

    // —— kui Paint 同名兼容别名（取官方语义最近值，供既有代码平滑迁移）——
    pub ink: Hsla,       // = label_primary
    pub secondary: Hsla, // = label_secondary
    pub faint: Hsla,     // = label_tertiary
    pub line: Hsla,      // = border_l2
    pub danger: Hsla,    // = state_error_primary
    pub success: Hsla,   // = state_success_primary
    pub stage: Hsla,     // = bg_layer_1
    pub panel: Hsla,     // = bg_layer_2
    pub tray: Hsla,      // = menu_surface
    pub bright: Hsla,    // = button_info_fill
}

impl Tokens {
    pub fn for_mode(mode: ThemeMode) -> Self {
        if mode == ThemeMode::Dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_base: rgb(0x151517).into(),
            bg_layer_1: rgb(0x232324).into(),
            bg_layer_2: rgb(0x2c2c2e).into(),
            bg_layer_3: rgb(0x353638).into(),
            bg_mask_1: rgba(0x00000080).into(),
            bg_module_platform: rgb(0x353638).into(),
            bg_overlay: rgb(0x61666b).into(),
            sidebar_fill: rgb(0x1b1b1c).into(),
            sidebar_nav_active: rgb(0x43454a).into(),
            sidebar_nav_active_accent: rgb(0x353638).into(),
            input_major: rgb(0x2c2c2e).into(),
            login_input: rgb(0x1b1b1c).into(),
            bubble: rgb(0x2c2c2e).into(),
            selector: rgb(0x353638).into(),
            tip: rgb(0x353638).into(),
            border_l1: rgba(0xffffff0f).into(),
            border_l2: rgba(0xffffff1f).into(),
            border_l3: rgba(0xffffff29).into(),
            border_l4: rgba(0xffffff33).into(),
            label_primary: rgb(0xf9fafb).into(),
            label_secondary: rgb(0xcfd3d6).into(),
            label_tertiary: rgb(0xadb2b8).into(),
            label_caption: rgb(0x81858c).into(),
            label_dimmed: rgb(0x43454a).into(),
            label_primary_inverted: rgb(0x353638).into(),
            label_primary_foreground: rgb(0x0f1115).into(),
            label_primary_bluish: rgb(0xf9fafb).into(),
            hover: rgba(0xffffff14).into(),
            hover_active: rgba(0xffffff24).into(),
            hover_accent: rgba(0xffffff3d).into(),
            hover_danger: rgba(0xf25a5a26).into(),
            hover_solid: rgb(0x353638).into(),
            button_primary_fill: rgb(0xf9fafb).into(),
            button_primary_hover: rgb(0xebeef2).into(),
            button_elevated: rgb(0x43454a).into(),
            button_floating: rgb(0x2c2c2e).into(),
            button_floating_hover: rgb(0x353638).into(),
            button_ghost_active: rgb(0x43454a).into(),
            button_info_fill: rgb(0x7aaaff).into(),
            button_info_hover: rgb(0x4176e6).into(),
            state_business_primary: rgb(0x7aaaff).into(),
            state_business_tertiary: rgb(0x34415b).into(),
            state_success_primary: rgb(0x22c55e).into(),
            state_success_secondary: rgb(0x4ed17e).into(),
            state_success_tertiary: rgb(0x233c2c).into(),
            state_error_primary: rgb(0xf25a5a).into(),
            state_error_secondary: rgb(0xf25a5a).into(),
            state_warn_primary: rgb(0xf59e0b).into(),
            state_warn_label: rgb(0xdd8629).into(),
            state_warn_tertiary: rgb(0x27241f).into(),
            state_idle_primary: rgb(0x545557).into(),
            link: rgb(0x7aaaff).into(),
            brand_accent: rgb(0x5686fe).into(),
            menu_stroke: rgba(0xffffff29).into(),
            menu_surface_fill: Hsla::from(rgb(0x303136)).alpha(0.64),
            menu_surface: rgb(0x303136).into(),
            menu_group_header: rgb(0x303136).into(),
            toast_bg: rgb(0x43454a).into(),
            toast_label: rgb(0xffffff).into(),
            tooltip_bg: rgb(0x43454a).into(),
            scrollbar_l1: rgb(0x3c3c3d).into(),
            scrollbar_l1_hover: rgb(0x545557).into(),
            scrollbar_l2: rgb(0x545557).into(),
            scrollbar_l2_hover: rgb(0x65676b).into(),
            md_code_block: rgb(0x1b1b1c).into(),
            md_code_block_banner: rgb(0x2c2c2e).into(),
            md_inline_code: rgb(0x292929).into(),
            md_tag: rgb(0x2c2c2e).into(),
            switch_thumb: rgb(0xadb2b8).into(),
            ink: rgb(0xf9fafb).into(),
            secondary: rgb(0xcfd3d6).into(),
            faint: rgb(0xadb2b8).into(),
            line: rgba(0xffffff1f).into(),
            danger: rgb(0xf25a5a).into(),
            success: rgb(0x22c55e).into(),
            stage: rgb(0x232324).into(),
            panel: rgb(0x2c2c2e).into(),
            tray: rgb(0x303136).into(),
            bright: rgb(0x7aaaff).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            bg_base: rgb(0xffffff).into(),
            bg_layer_1: rgb(0xffffff).into(),
            bg_layer_2: rgb(0xffffff).into(),
            bg_layer_3: rgb(0xffffff).into(),
            bg_mask_1: rgba(0x0000003d).into(),
            bg_module_platform: rgb(0xf5f6f7).into(),
            bg_overlay: rgb(0xe9ecf2).into(),
            sidebar_fill: rgb(0xf9fafb).into(),
            sidebar_nav_active: rgb(0xebeef2).into(),
            sidebar_nav_active_accent: rgb(0xe4edfd).into(),
            input_major: rgb(0xffffff).into(),
            login_input: rgb(0xf9fafb).into(),
            bubble: rgb(0xedf3fe).into(),
            selector: rgb(0xf5f6f7).into(),
            tip: rgb(0xf5f6f7).into(),
            border_l1: rgba(0x0000000a).into(),
            border_l2: rgba(0x0000001a).into(),
            border_l3: rgba(0x0000001f).into(),
            border_l4: rgba(0x00000029).into(),
            label_primary: rgb(0x0f1115).into(),
            label_secondary: rgb(0x61666b).into(),
            label_tertiary: rgb(0x81858c).into(),
            label_caption: rgb(0xadb2b8).into(),
            label_dimmed: rgb(0xe1e5ee).into(),
            label_primary_inverted: rgb(0xffffff).into(),
            label_primary_foreground: rgb(0xffffff).into(),
            label_primary_bluish: rgb(0x0e3074).into(),
            hover: rgba(0x2631480f).into(),
            hover_active: rgba(0x2631481a).into(),
            hover_accent: rgba(0x26314824).into(),
            hover_danger: rgba(0xec13130d).into(),
            hover_solid: rgb(0xf1f3f5).into(),
            button_primary_fill: rgb(0x0f1115).into(),
            button_primary_hover: rgb(0x43454a).into(),
            button_elevated: rgb(0xffffff).into(),
            button_floating: rgb(0xffffff).into(),
            button_floating_hover: rgb(0xf1f3f5).into(),
            button_ghost_active: rgb(0xebeef2).into(),
            button_info_fill: rgb(0x4176e6).into(),
            button_info_hover: rgb(0x7aaaff).into(),
            state_business_primary: rgb(0x4176e6).into(),
            state_business_tertiary: rgb(0xe4edfd).into(),
            state_success_primary: rgb(0x22c55e).into(),
            state_success_secondary: rgb(0x4ed17e).into(),
            state_success_tertiary: rgb(0xe6faed).into(),
            state_error_primary: rgb(0xec1313).into(),
            state_error_secondary: rgb(0xf25a5a).into(),
            state_warn_primary: rgb(0xf59e0b).into(),
            state_warn_label: rgb(0xdd8629).into(),
            state_warn_tertiary: rgb(0xfef5e7).into(),
            state_idle_primary: rgb(0xd4d4d4).into(),
            link: rgb(0x4176e6).into(),
            brand_accent: rgb(0x4176e6).into(),
            menu_stroke: rgba(0x0000000a).into(),
            menu_surface_fill: Hsla::from(rgb(0xfcfdfe)).alpha(0.7),
            menu_surface: rgb(0xf8f9fa).into(),
            menu_group_header: rgb(0xf8f9fa).into(),
            toast_bg: rgb(0x353638).into(),
            toast_label: rgb(0xffffff).into(),
            tooltip_bg: rgb(0x2c2c2e).into(),
            scrollbar_l1: rgb(0xe5e5e5).into(),
            scrollbar_l1_hover: rgb(0xd4d4d4).into(),
            scrollbar_l2: rgb(0xe5e5e5).into(),
            scrollbar_l2_hover: rgb(0xd4d4d4).into(),
            md_code_block: rgb(0xf9fafb).into(),
            md_code_block_banner: rgb(0xf9fafb).into(),
            md_inline_code: rgb(0xfafafa).into(),
            md_tag: rgb(0xf1f3f5).into(),
            switch_thumb: rgb(0xffffff).into(),
            ink: rgb(0x0f1115).into(),
            secondary: rgb(0x61666b).into(),
            faint: rgb(0x81858c).into(),
            line: rgba(0x0000001a).into(),
            danger: rgb(0xec1313).into(),
            success: rgb(0x22c55e).into(),
            stage: rgb(0xffffff).into(),
            panel: rgb(0xffffff).into(),
            tray: rgb(0xf8f9fa).into(),
            bright: rgb(0x4176e6).into(),
        }
    }

    /// 官方圆角（--dsw-radius-*）。
    pub const RADIUS_XS: f32 = 4.;
    pub const RADIUS_SM: f32 = 8.;
    pub const RADIUS_MD: f32 = 12.;
    pub const RADIUS_LG: f32 = 16.;
    pub const RADIUS_XL: f32 = 20.;
    pub const RADIUS_PANEL: f32 = 28.;
}

/// 按当前主题取官方 token。
pub fn dsh(cx: &App) -> Tokens {
    Tokens::for_mode(Theme::global(cx).mode)
}

// —— 官方阴影（--dsw-elevation-* / --dsw-shadow-lv3），发丝描边由 .border(px(0.5)) 承担 ——

fn shadow(color: u32, oy: f32, blur: f32, spread: f32) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: gpui::rgba(color).into(),
        offset: gpui::Point {
            x: gpui::px(0.),
            y: gpui::px(oy),
        },
        blur_radius: gpui::px(blur),
        spread_radius: gpui::px(spread),
        inset: false,
    }
}

/// --dsw-elevation-soft：composer 卡（配合 .5px border-l2）。
pub fn elevation_soft() -> Vec<gpui::BoxShadow> {
    vec![
        shadow(0x00000008, 4., 16., 0.),
        shadow(0x00000008, 0., 24., 0.),
    ]
}

/// --dsw-elevation-panel：浮层面板。
pub fn elevation_panel() -> Vec<gpui::BoxShadow> {
    vec![
        shadow(0x00000008, 3., 8., 0.),
        shadow(0x00000005, 0., 16., 0.),
    ]
}

/// --dsw-elevation-prominent：菜单/模态（配合 .5px border-l1）。
pub fn elevation_prominent() -> Vec<gpui::BoxShadow> {
    vec![
        shadow(0x0000000a, 3., 8., 0.),
        shadow(0x0000000d, 0., 20., 0.),
    ]
}

/// --dsw-shadow-lv3：toast。
pub fn shadow_lv3() -> Vec<gpui::BoxShadow> {
    vec![
        shadow(0x00000030, 0., 1., 0.),
        shadow(0x00000005, 0., 4., 0.),
        shadow(0x00000014, 12., 32., 0.),
    ]
}
