//! Official `ui-theme` preference: light, dark or follow-the-system.
//!
//! The persisted value is the user's preference, never the resolved theme: a
//! "跟随系统" choice keeps following the system appearance after restarts and
//! while the OS switches, exactly like the official Appearance row.

/// One of the official three appearance preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    Light,
    Dark,
    /// Official `DEFAULT_PREFERENCE`; also the fallback for unknown ids.
    #[default]
    System,
}

impl ThemePreference {
    /// Every preference in the official cube order (light, dark, system).
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    /// Stable id persisted in `workspaces.json` (`themeMode`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    /// Official simplified-Chinese labels (`appearance.light` etc).
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "浅色",
            Self::Dark => "深色",
            Self::System => "跟随系统",
        }
    }

    /// Parse a persisted id; unknown values fall back to the official default.
    pub fn from_id(id: &str) -> Self {
        match id.trim().to_lowercase().as_str() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    /// Whether this preference currently resolves to the dark theme.
    pub fn applies_dark(self, system_dark: bool) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => system_dark,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ThemePreference;

    #[test]
    fn ids_round_trip_and_unknown_falls_back_to_system() {
        for preference in ThemePreference::ALL {
            assert_eq!(ThemePreference::from_id(preference.id()), preference);
        }
        assert_eq!(ThemePreference::from_id("DARK"), ThemePreference::Dark);
        assert_eq!(
            ThemePreference::from_id(" system "),
            ThemePreference::System
        );
        assert_eq!(ThemePreference::from_id("sepia"), ThemePreference::System);
        assert_eq!(ThemePreference::default(), ThemePreference::System);
        assert_eq!(
            ThemePreference::ALL.map(ThemePreference::id),
            ["light", "dark", "system"]
        );
    }

    #[test]
    fn system_preference_follows_the_platform_appearance() {
        for dark in [false, true] {
            assert!(!ThemePreference::Light.applies_dark(dark));
            assert!(ThemePreference::Dark.applies_dark(dark));
            assert_eq!(ThemePreference::System.applies_dark(dark), dark);
        }
    }
}
