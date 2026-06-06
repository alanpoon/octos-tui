//! First-run setup wizard step model.
//!
//! The onboarding flow is rendered by two menu providers
//! (`onboarding_local_profile_menu` and `onboarding_provider_setup_menu` in
//! `providers.rs`), but to the user it is ONE guided wizard. This module
//! computes a single, content-agnostic projection of the wizard's progress
//! from [`OnboardingWizardState`] so each onboarding screen can show:
//!
//!   * a `Step N of M` header,
//!   * a one-line purpose for the current step ("why am I here"),
//!   * the next concrete action ("what do I do / what's next"),
//!   * a right-side checklist of every step with its completion mark.
//!
//! Keeping this in one place means the two providers stay in lock-step and the
//! progress copy is computed, not duplicated.
//!
//! NOTE (i18n): the user-facing strings here live in `locales/{en,zh}.yml`
//! under the `onboarding.wizard.*` namespace and are resolved via `t!()`. CJK
//! width is handled by the generic menu render surface (`unicode-width`), so no
//! width math lives in this module — never compare against a rendered string.

use std::borrow::Cow;

use crate::menu::types::{MenuPreview, MenuPreviewRow};
use crate::model::{
    OnboardingProviderStatus, OnboardingWizardState, OnboardingWorkspaceValidation,
};

/// A single user-facing wizard step. The internal provider menus may contain
/// more granular rows (e.g. family vs. model vs. route), but the wizard
/// presents them grouped into these coarse, explainable steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Create the local profile (name / username / email).
    Profile,
    /// Choose the model family + model + provider route.
    Provider,
    /// Enter the API key and verify the route with a live test.
    Connect,
    /// Save the verified provider into the profile JSON.
    Save,
    /// Stage + validate the workspace folder the agent will code in.
    Workspace,
    /// Open the coding session and drop into the working surface.
    Activate,
}

impl WizardStep {
    /// Ordered list of every step, used for the checklist + N-of-M math.
    pub const ALL: [WizardStep; 6] = [
        WizardStep::Profile,
        WizardStep::Provider,
        WizardStep::Connect,
        WizardStep::Save,
        WizardStep::Workspace,
        WizardStep::Activate,
    ];

    /// 1-based ordinal for "Step N of M".
    pub fn number(self) -> usize {
        Self::ALL
            .iter()
            .position(|step| *step == self)
            .map(|index| index + 1)
            .unwrap_or(1)
    }

    /// Stable i18n key suffix for this step (NOT user-facing text). Used to
    /// build `onboarding.wizard.step.*` / `onboarding.wizard.purpose.*` keys
    /// without ever switching on a translated string.
    fn key(self) -> &'static str {
        match self {
            WizardStep::Profile => "profile",
            WizardStep::Provider => "provider",
            WizardStep::Connect => "connect",
            WizardStep::Save => "save",
            WizardStep::Workspace => "workspace",
            WizardStep::Activate => "activate",
        }
    }

    /// Short checklist label (right-side panel).
    pub fn short_title(self) -> Cow<'static, str> {
        t!(format!("onboarding.wizard.step.{}", self.key()))
    }

    /// One-line purpose ("why this step exists").
    pub fn purpose(self) -> Cow<'static, str> {
        t!(format!("onboarding.wizard.purpose.{}", self.key()))
    }
}

/// Computed snapshot of the wizard's progress, derived from the wizard state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardProgress {
    pub current: WizardStep,
    /// Completion mark per step, in [`WizardStep::ALL`] order.
    pub done: [bool; 6],
}

impl WizardProgress {
    /// Derive progress from the wizard state. `local_create_supported` selects
    /// whether the Profile step is part of the flow (solo/local mode) or
    /// implicitly satisfied by an already-resolved server profile.
    pub fn from_state(
        state: &OnboardingWizardState,
        current_profile: Option<&str>,
        local_create_supported: bool,
    ) -> Self {
        let profile_done = state.effective_profile_id(current_profile).is_some()
            || (!local_create_supported && current_profile.is_some());
        let provider_done = state.selection_ready();
        let connect_done = state.provider_tested
            || matches!(
                state.provider_status(),
                OnboardingProviderStatus::SavedPrimary
            );
        let save_done = matches!(
            state.provider_status(),
            OnboardingProviderStatus::SavedPrimary | OnboardingProviderStatus::SavedFallback
        );
        let workspace_done = matches!(
            state.workspace_validation,
            OnboardingWorkspaceValidation::Valid { .. }
        );
        // Activate is only "done" once the session is open, which tears the
        // wizard down — so within the wizard it is never marked complete.
        let activate_done = false;

        let done = [
            profile_done,
            provider_done,
            connect_done,
            save_done,
            workspace_done,
            activate_done,
        ];

        // The current step is the first incomplete one (Activate is the
        // terminal step once everything before it is done).
        let current = WizardStep::ALL
            .iter()
            .zip(done.iter())
            .find(|(_, complete)| !**complete)
            .map(|(step, _)| *step)
            .unwrap_or(WizardStep::Activate);

        Self { current, done }
    }

    /// `Step N of M — <Short title>` header for the menu subtitle.
    pub fn header(&self) -> String {
        t!(
            "onboarding.wizard.header",
            number = self.current.number(),
            total = WizardStep::ALL.len(),
            title = self.current.short_title(),
        )
        .into_owned()
    }

    /// Full subtitle: header + one-line purpose of the current step.
    pub fn subtitle(&self) -> String {
        t!(
            "onboarding.wizard.subtitle",
            header = self.header(),
            purpose = self.current.purpose(),
        )
        .into_owned()
    }

    /// Footer hint naming the next concrete action.
    pub fn footer_hint(&self, next_action: &str) -> String {
        t!("onboarding.wizard.footer", next = next_action).into_owned()
    }

    /// Right-side checklist preview: one row per step, current marked `>`,
    /// completed marked `[x]`, pending `[ ]`.
    pub fn checklist_preview(&self) -> MenuPreview {
        let rows = WizardStep::ALL
            .iter()
            .zip(self.done.iter())
            .map(|(step, &complete)| {
                let marker = if *step == self.current {
                    ">"
                } else if complete {
                    "[x]"
                } else {
                    "[ ]"
                };
                MenuPreviewRow {
                    label: format!("{marker} {}", step.number()),
                    value: step.short_title().into_owned(),
                }
            })
            .collect();
        MenuPreview::KeyValues {
            title: Some(t!("onboarding.wizard.progress_title").into_owned()),
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OnboardingWorkspaceValidation;

    fn valid_workspace() -> OnboardingWorkspaceValidation {
        OnboardingWorkspaceValidation::Valid {
            canonical: "/tmp/ws".into(),
            writable: true,
            has_workspace_toml: false,
        }
    }

    /// A wizard state with a resolved profile (step 1 complete).
    fn state_with_profile() -> OnboardingWizardState {
        OnboardingWizardState {
            profile_id: Some("alice".into()),
            ..OnboardingWizardState::default()
        }
    }

    /// A wizard state with profile + a ready provider selection (steps 1-2).
    fn state_with_selection() -> OnboardingWizardState {
        let mut state = state_with_profile();
        state.provider.family_id = "gpt".into();
        state.provider.model_id = "gpt-x".into();
        state.provider.route.route_id = "openai".into();
        state
    }

    #[test]
    fn fresh_state_starts_on_profile_step() {
        let state = OnboardingWizardState::default();
        let progress = WizardProgress::from_state(&state, None, true);
        assert_eq!(progress.current, WizardStep::Profile);
        assert_eq!(progress.current.number(), 1);
        // Assert via the same i18n key (NOT a hardcoded English literal) so the
        // test tracks the source string across locales/wording changes. The
        // step is 1-of-6 and names the Profile step's short title.
        assert_eq!(
            progress.header(),
            t!(
                "onboarding.wizard.header",
                number = 1,
                total = 6,
                title = WizardStep::Profile.short_title(),
            )
        );
        assert!(progress.done.iter().all(|done| !done));
    }

    #[test]
    fn resolved_profile_advances_to_provider_step() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true);
        assert_eq!(progress.current, WizardStep::Provider);
        assert!(progress.done[0]);
    }

    #[test]
    fn ready_selection_advances_to_connect_step() {
        let progress = WizardProgress::from_state(&state_with_selection(), None, true);
        assert_eq!(progress.current, WizardStep::Connect);
        assert!(progress.done[1], "provider step complete");
    }

    #[test]
    fn validated_workspace_with_save_lands_on_activate() {
        let mut state = state_with_selection();
        state.provider_tested = true;
        state.provider_saved = true;
        state.workspace_validation = valid_workspace();

        let progress = WizardProgress::from_state(&state, None, true);
        assert_eq!(progress.current, WizardStep::Activate);
        assert!(progress.done[..5].iter().all(|done| *done));
        assert!(!progress.done[5], "activate never self-marks complete");
    }

    #[test]
    fn checklist_marks_current_completed_and_pending() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true);
        let MenuPreview::KeyValues { rows, .. } = progress.checklist_preview() else {
            panic!("expected key-value checklist");
        };
        assert_eq!(rows.len(), 6);
        assert!(rows[0].label.starts_with("[x]"), "profile done");
        assert!(rows[1].label.starts_with('>'), "provider current");
        assert!(rows[2].label.starts_with("[ ]"), "connect pending");
    }

    #[test]
    fn server_profile_without_local_create_skips_profile_step() {
        let state = OnboardingWizardState::default();
        let progress = WizardProgress::from_state(&state, Some("server-prof"), false);
        assert!(
            progress.done[0],
            "server-authenticated profile satisfies step 1"
        );
        assert_eq!(progress.current, WizardStep::Provider);
    }
}
