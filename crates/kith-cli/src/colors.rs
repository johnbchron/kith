//! Semantic colour palette for the kith TUI.
#![allow(dead_code)] // Phase A only uses a subset; remainder is reserved for B/C.
//!
//! All colours are sourced from the Radix UI colour system via
//! [`radix_colors_rs`]. The two primary palettes are:
//!
//! - **OrangeDark** (`ORANGEDARK_ORANGE{N}`) — accent / interactive elements
//! - **SandDark** (`SANDDARK_SAND{N}`) — neutral chrome and text
//!
//! Other Radix dark palettes are used for semantic status colours.
//!
//! Radix dark scale semantics (1 = darkest background, 12 = brightest text):
//! - 1–2  App / subtle backgrounds
//! - 3–5  Component backgrounds (hover, active, selected)
//! - 6–8  Borders and separators
//! - 9–10 Solid fills (badges, buttons)
//! - 11   Low-contrast / muted text
//! - 12   High-contrast text

use radix_colors_rs::ColorU8;
use ratatui::style::{Color, Modifier, Style};

// ─── Conversion helper ────────────────────────────────────────────────────────

#[inline]
fn rgb(c: ColorU8) -> Color { Color::Rgb(c.r, c.g, c.b) }

// ─── Backgrounds ─────────────────────────────────────────────────────────────

/// Full-screen application background (darkest sand — scale step 1).
/// Painted across the entire frame on every draw to prevent the native
/// terminal colour from bleeding through.
pub fn app_bg() -> Color { rgb(radix_colors_rs::SANDDARK_SAND1) }

/// Panel / widget interior background (scale step 2 — just above app bg).
pub fn panel_bg() -> Color { rgb(radix_colors_rs::SANDDARK_SAND2) }

// ─── Neutral — SandDark ───────────────────────────────────────────────────────

/// High-contrast body text.
pub fn text() -> Color { rgb(radix_colors_rs::SANDDARK_SAND12) }

/// Muted / secondary text (hints, labels, status bar).
pub fn text_muted() -> Color { rgb(radix_colors_rs::SANDDARK_SAND11) }

/// Subtle text for de-emphasised content (e.g. superseded facts).
pub fn text_subtle() -> Color { rgb(radix_colors_rs::SANDDARK_SAND10) }

/// Normal border / separator colour.
pub fn border() -> Color { rgb(radix_colors_rs::SANDDARK_SAND7) }

/// Border for the active / focused pane.
pub fn border_focus() -> Color { rgb(radix_colors_rs::SANDDARK_SAND8) }

// ─── Accent — OrangeDark ─────────────────────────────────────────────────────

/// Solid accent fill — used as the mode-badge background.
pub fn accent_solid() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE9) }

/// Foreground to place on top of [`accent_solid`] (very dark orange).
pub fn accent_on_solid() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE1) }

/// Mid-weight accent text — fact-type labels, selected contact name.
pub fn accent_text() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE11) }

/// High-contrast accent text — contact name in the detail title.
pub fn accent_hi() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE12) }

/// Subtle accent background — selected / cursor row in the list.
pub fn accent_bg() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE4) }

/// Slightly brighter accent bg — hovered row or secondary selection.
pub fn accent_bg_hover() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE5) }

/// Very subtle accent tint — filter-bar highlight.
pub fn accent_subtle() -> Color { rgb(radix_colors_rs::ORANGEDARK_ORANGE3) }

// ─── Semantic status ─────────────────────────────────────────────────────────

/// Text colour for retracted facts (RedDark 11).
pub fn retracted() -> Color { rgb(radix_colors_rs::REDDARK_RED11) }

/// Text colour for superseded facts (dimmed neutral).
pub fn superseded() -> Color { rgb(radix_colors_rs::SANDDARK_SAND10) }

/// Confidence badge for `Probable` facts (YellowDark 11).
pub fn probable() -> Color { rgb(radix_colors_rs::YELLOWDARK_YELLOW11) }

/// Confidence badge for `Rumored` facts (AmberDark 11).
pub fn rumored() -> Color { rgb(radix_colors_rs::AMBERDARK_AMBER11) }

/// Filter prompt colour (YellowDark 11 — stands out without being alarming).
pub fn filter_prompt() -> Color { rgb(radix_colors_rs::YELLOWDARK_YELLOW11) }

// ─── Convenience Style builders ──────────────────────────────────────────────

pub fn style_text() -> Style { Style::default().fg(text()) }
pub fn style_muted() -> Style { Style::default().fg(text_muted()) }
pub fn style_subtle() -> Style { Style::default().fg(text_subtle()) }
pub fn style_border() -> Style { Style::default().fg(border()) }
pub fn style_border_focus() -> Style { Style::default().fg(border_focus()) }

pub fn style_accent_text() -> Style { Style::default().fg(accent_text()).add_modifier(Modifier::BOLD) }
pub fn style_accent_hi() -> Style { Style::default().fg(accent_hi()).add_modifier(Modifier::BOLD) }

/// Style for a selected / cursor row.
pub fn style_selected() -> Style {
  Style::default()
    .bg(accent_bg())
    .fg(accent_hi())
    .add_modifier(Modifier::BOLD)
}

/// Style for the mode badge (e.g. `NORMAL`, `DETAIL`).
pub fn style_mode_badge() -> Style {
  Style::default()
    .bg(accent_solid())
    .fg(accent_on_solid())
    .add_modifier(Modifier::BOLD)
}
