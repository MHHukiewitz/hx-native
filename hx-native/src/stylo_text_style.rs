//! Stylo → Parley `TextStyle` conversion for ellipsis reshape (issue #12).
//!
//! Copied from Blitz 0.3.0-beta.4 `blitz_dom::stylo_to_parley` (which is
//! `pub(crate)` and unreachable from hx-native). Listed under pin sensitivity
//! in [`crate::ellipsis`]: a Blitz/stylo bump can change this silently.
//!
//! Keep this file aligned with Blitz's copy when bumping the Blitz pin.

use std::borrow::Cow;

use blitz_dom::node::TextBrush;
use blitz_dom::NodeId;
use style::values::computed::Length;

// Module of type aliases so we can refer to stylo types with nicer names
mod stylo {
    pub(super) use style::computed_values::direction::T as Direction;
    pub(super) use style::computed_values::font_variant_caps::T as FontVariantCaps;
    pub(super) use style::computed_values::font_variant_position::T as FontVariantPosition;
    pub(super) use style::computed_values::text_wrap_mode::T as TextWrapMode;
    pub(super) use style::computed_values::unicode_bidi::T as UnicodeBidi;
    pub(super) use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
    pub(super) use style::properties::style_structs::Font;
    pub(super) use style::properties::ComputedValues;
    pub(super) use style::values::computed::font::FontFeatureSettings;
    pub(super) use style::values::computed::font::FontStyle;
    pub(super) use style::values::computed::font::FontVariantEastAsian;
    pub(super) use style::values::computed::font::FontVariantLigatures;
    pub(super) use style::values::computed::font::FontVariantNumeric;
    pub(super) use style::values::computed::font::FontVariationSettings;
    pub(super) use style::values::computed::font::FontWeight;
    pub(super) use style::values::computed::font::FontWidth;
    pub(super) use style::values::computed::font::GenericFontFamily;
    pub(super) use style::values::computed::font::LineHeight;
    pub(super) use style::values::computed::font::SingleFontFamily;
    pub(super) use style::values::computed::AlignmentBaseline;
    pub(super) use style::values::computed::BaselineShift;
    pub(super) use style::values::computed::LineBreak;
    pub(super) use style::values::computed::OverflowWrap;
    pub(super) use style::values::computed::WordBreak;
    pub(super) use style::values::generics::box_::BaselineShiftKeyword;
}

mod parley_types {
    pub(super) use parley::setting::Tag;
    pub(super) use parley::style::*;
    pub(super) use parley::FontFeature;
    pub(super) use parley::FontVariation;
    pub(super) use parley::Language;
}

fn generic_font_family(input: stylo::GenericFontFamily) -> parley_types::GenericFamily {
    match input {
        stylo::GenericFontFamily::None => parley_types::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Serif => parley_types::GenericFamily::Serif,
        stylo::GenericFontFamily::SansSerif => parley_types::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Monospace => parley_types::GenericFamily::Monospace,
        stylo::GenericFontFamily::Cursive => parley_types::GenericFamily::Cursive,
        stylo::GenericFontFamily::Fantasy => parley_types::GenericFamily::Fantasy,
        stylo::GenericFontFamily::SystemUi => parley_types::GenericFamily::SystemUi,
    }
}

fn font_weight(input: stylo::FontWeight) -> parley_types::FontWeight {
    parley_types::FontWeight::new(input.value())
}

fn font_width(input: stylo::FontWidth) -> parley_types::FontWidth {
    parley_types::FontWidth::from_percentage(input.0.to_float())
}

fn font_style(input: stylo::FontStyle) -> parley_types::FontStyle {
    match input {
        stylo::FontStyle::NORMAL => parley_types::FontStyle::Normal,
        stylo::FontStyle::ITALIC => parley_types::FontStyle::Italic,
        val => parley_types::FontStyle::Oblique(Some(val.oblique_degrees())),
    }
}

fn font_variations(input: &stylo::FontVariationSettings) -> Vec<parley_types::FontVariation> {
    input
        .0
        .iter()
        .map(|v| parley_types::FontVariation {
            tag: parley_types::Tag::from_bytes(v.tag.0.to_be_bytes()),
            value: v.value,
        })
        .collect()
}

#[inline]
fn feature(tag: &[u8; 4], value: u16) -> parley_types::FontFeature {
    parley_types::FontFeature {
        tag: parley_types::Tag::from_bytes(*tag),
        value,
    }
}

fn font_feature_settings(
    input: &stylo::FontFeatureSettings,
    out: &mut Vec<parley_types::FontFeature>,
) {
    out.extend(input.0.iter().map(|v| parley_types::FontFeature {
        tag: parley_types::Tag::from_bytes(v.tag.0.to_be_bytes()),
        value: v.value as u16,
    }));
}

fn font_variant_ligatures(
    input: stylo::FontVariantLigatures,
    out: &mut Vec<parley_types::FontFeature>,
) {
    use stylo::FontVariantLigatures as L;

    if input.contains(L::NONE) {
        out.push(feature(b"liga", 0));
        out.push(feature(b"clig", 0));
        out.push(feature(b"dlig", 0));
        out.push(feature(b"hlig", 0));
        out.push(feature(b"calt", 0));
        return;
    }

    if input.contains(L::COMMON_LIGATURES) {
        out.push(feature(b"liga", 1));
        out.push(feature(b"clig", 1));
    } else if input.contains(L::NO_COMMON_LIGATURES) {
        out.push(feature(b"liga", 0));
        out.push(feature(b"clig", 0));
    }
    if input.contains(L::DISCRETIONARY_LIGATURES) {
        out.push(feature(b"dlig", 1));
    } else if input.contains(L::NO_DISCRETIONARY_LIGATURES) {
        out.push(feature(b"dlig", 0));
    }
    if input.contains(L::HISTORICAL_LIGATURES) {
        out.push(feature(b"hlig", 1));
    } else if input.contains(L::NO_HISTORICAL_LIGATURES) {
        out.push(feature(b"hlig", 0));
    }
    if input.contains(L::CONTEXTUAL) {
        out.push(feature(b"calt", 1));
    } else if input.contains(L::NO_CONTEXTUAL) {
        out.push(feature(b"calt", 0));
    }
}

fn font_variant_caps(input: stylo::FontVariantCaps, out: &mut Vec<parley_types::FontFeature>) {
    match input {
        stylo::FontVariantCaps::Normal => {}
        stylo::FontVariantCaps::SmallCaps => out.push(feature(b"smcp", 1)),
    }
}

fn font_variant_position(
    input: stylo::FontVariantPosition,
    out: &mut Vec<parley_types::FontFeature>,
) {
    match input {
        stylo::FontVariantPosition::Normal => {}
        stylo::FontVariantPosition::Sub => out.push(feature(b"subs", 1)),
        stylo::FontVariantPosition::Super => out.push(feature(b"sups", 1)),
    }
}

fn font_variant_numeric(
    input: stylo::FontVariantNumeric,
    out: &mut Vec<parley_types::FontFeature>,
) {
    use stylo::FontVariantNumeric as N;

    if input.contains(N::LINING_NUMS) {
        out.push(feature(b"lnum", 1));
    }
    if input.contains(N::OLDSTYLE_NUMS) {
        out.push(feature(b"onum", 1));
    }
    if input.contains(N::PROPORTIONAL_NUMS) {
        out.push(feature(b"pnum", 1));
    }
    if input.contains(N::TABULAR_NUMS) {
        out.push(feature(b"tnum", 1));
    }
    if input.contains(N::DIAGONAL_FRACTIONS) {
        out.push(feature(b"frac", 1));
    }
    if input.contains(N::STACKED_FRACTIONS) {
        out.push(feature(b"afrc", 1));
    }
    if input.contains(N::ORDINAL) {
        out.push(feature(b"ordn", 1));
    }
    if input.contains(N::SLASHED_ZERO) {
        out.push(feature(b"zero", 1));
    }
}

fn font_variant_east_asian(
    input: stylo::FontVariantEastAsian,
    out: &mut Vec<parley_types::FontFeature>,
) {
    use stylo::FontVariantEastAsian as E;

    if input.contains(E::JIS78) {
        out.push(feature(b"jp78", 1));
    }
    if input.contains(E::JIS83) {
        out.push(feature(b"jp83", 1));
    }
    if input.contains(E::JIS90) {
        out.push(feature(b"jp90", 1));
    }
    if input.contains(E::JIS04) {
        out.push(feature(b"jp04", 1));
    }
    if input.contains(E::SIMPLIFIED) {
        out.push(feature(b"smpl", 1));
    }
    if input.contains(E::TRADITIONAL) {
        out.push(feature(b"trad", 1));
    }
    if input.contains(E::FULL_WIDTH) {
        out.push(feature(b"fwid", 1));
    }
    if input.contains(E::PROPORTIONAL_WIDTH) {
        out.push(feature(b"pwid", 1));
    }
    if input.contains(E::RUBY) {
        out.push(feature(b"ruby", 1));
    }
}

fn font_features(font_styles: &stylo::Font) -> Vec<parley_types::FontFeature> {
    let mut features = Vec::new();
    font_variant_ligatures(font_styles.font_variant_ligatures, &mut features);
    font_variant_caps(font_styles.font_variant_caps, &mut features);
    font_variant_numeric(font_styles.font_variant_numeric, &mut features);
    font_variant_east_asian(font_styles.font_variant_east_asian, &mut features);
    font_variant_position(font_styles.font_variant_position, &mut features);
    font_feature_settings(&font_styles.font_feature_settings, &mut features);
    features.sort_by_key(|feature| feature.tag);
    features.dedup_by_key(|feature| feature.tag);
    features
}

fn text_wrap_mode(input: stylo::TextWrapMode) -> parley_types::TextWrapMode {
    match input {
        stylo::TextWrapMode::Wrap => parley_types::TextWrapMode::Wrap,
        stylo::TextWrapMode::Nowrap => parley_types::TextWrapMode::NoWrap,
    }
}

fn white_space_collapse(input: stylo::WhiteSpaceCollapse) -> parley_types::WhiteSpaceCollapse {
    match input {
        stylo::WhiteSpaceCollapse::Collapse => parley_types::WhiteSpaceCollapse::Collapse,
        stylo::WhiteSpaceCollapse::Preserve => parley_types::WhiteSpaceCollapse::Preserve,
        stylo::WhiteSpaceCollapse::PreserveBreaks => {
            parley_types::WhiteSpaceCollapse::PreserveBreaks
        }
        stylo::WhiteSpaceCollapse::BreakSpaces => parley_types::WhiteSpaceCollapse::BreakSpaces,
    }
}

fn vertical_align(style: &stylo::ComputedValues) -> parley_types::VerticalAlign {
    let box_styles = style.get_box();
    let alignment = match box_styles.clone_alignment_baseline() {
        stylo::AlignmentBaseline::Baseline => parley_types::AlignmentBaseline::Baseline,
        stylo::AlignmentBaseline::TextTop => parley_types::AlignmentBaseline::TextTop,
        stylo::AlignmentBaseline::TextBottom => parley_types::AlignmentBaseline::TextBottom,
        stylo::AlignmentBaseline::Middle => parley_types::AlignmentBaseline::Middle,
    };
    let shift = match box_styles.clone_baseline_shift() {
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Sub) => {
            parley_types::BaselineShift::Sub
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Super) => {
            parley_types::BaselineShift::Super
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Top) => {
            parley_types::BaselineShift::Top
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Bottom) => {
            parley_types::BaselineShift::Bottom
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Center) => {
            return parley_types::VerticalAlign::MIDDLE;
        }
        stylo::BaselineShift::Length(lp) => {
            let shift = if lp.has_percentage() {
                let font_styles = style.get_font();
                let font_size = font_styles.font_size.used_size.0.px();
                let line_height = match font_styles.line_height {
                    stylo::LineHeight::Normal => font_size * 1.2,
                    stylo::LineHeight::Number(num) => font_size * num.0,
                    stylo::LineHeight::Length(value) => value.0.px(),
                };
                lp.resolve(Length::new(line_height)).px()
            } else {
                lp.resolve(Length::new(0.0)).px()
            };
            parley_types::BaselineShift::Length(shift)
        }
    };
    parley_types::VerticalAlign::new(alignment, shift)
}

/// Build a parley [`TextStyle`] from an element's computed styles, matching
/// Blitz's `stylo_to_parley::style`.
pub(crate) fn text_style_from_computed(
    span_id: NodeId,
    style: &stylo::ComputedValues,
) -> parley::TextStyle<'static, 'static, TextBrush> {
    let font_styles = style.get_font();
    let itext_styles = style.get_inherited_text();

    let font_size = font_styles.font_size.used_size.0.px();
    let line_height = match font_styles.line_height {
        stylo::LineHeight::Normal => parley_types::LineHeight::NORMAL,
        stylo::LineHeight::Number(num) => parley_types::LineHeight::FontSizeRelative(num.0),
        stylo::LineHeight::Length(value) => parley_types::LineHeight::Absolute(value.0.px()),
    };
    let vertical_align = vertical_align(style);

    let letter_spacing = itext_styles
        .letter_spacing
        .0
        .resolve(Length::new(font_size))
        .px();

    let word_spacing = itext_styles
        .word_spacing
        .resolve(Length::new(font_size))
        .px();

    let font_weight = font_weight(font_styles.font_weight);
    let font_style = font_style(font_styles.font_style);
    let font_width = font_width(font_styles.font_width);
    let font_variations = font_variations(&font_styles.font_variation_settings);
    let font_features = font_features(font_styles);

    let families: Vec<_> = font_styles
        .font_family
        .families
        .list
        .iter()
        .map(|family| match family {
            stylo::SingleFontFamily::FamilyName(name) => {
                let name = name.name.as_str();
                #[cfg(target_vendor = "apple")]
                if name == "-apple-system" {
                    return parley_types::FontFamilyName::Generic(
                        parley_types::GenericFamily::SystemUi,
                    );
                }
                #[cfg(target_os = "macos")]
                if name == "BlinkMacSystemFont" {
                    return parley_types::FontFamilyName::Generic(
                        parley_types::GenericFamily::SystemUi,
                    );
                }
                parley_types::FontFamilyName::Named(Cow::Owned(name.to_string()))
            }
            stylo::SingleFontFamily::Generic(generic) => {
                parley_types::FontFamilyName::Generic(generic_font_family(*generic))
            }
        })
        .collect();

    let word_break = match itext_styles.word_break {
        stylo::WordBreak::Normal => parley_types::WordBreak::Normal,
        stylo::WordBreak::BreakAll => parley_types::WordBreak::BreakAll,
        stylo::WordBreak::KeepAll => parley_types::WordBreak::KeepAll,
    };
    let line_break = match itext_styles.line_break {
        stylo::LineBreak::Loose => parley_types::LineBreak::Loose,
        stylo::LineBreak::Normal => parley_types::LineBreak::Normal,
        stylo::LineBreak::Auto | stylo::LineBreak::Strict => parley_types::LineBreak::Strict,
        stylo::LineBreak::Anywhere => parley_types::LineBreak::Anywhere,
    };
    let overflow_wrap = match itext_styles.overflow_wrap {
        stylo::OverflowWrap::Normal => parley_types::OverflowWrap::Normal,
        stylo::OverflowWrap::BreakWord => parley_types::OverflowWrap::BreakWord,
        stylo::OverflowWrap::Anywhere => parley_types::OverflowWrap::Anywhere,
    };
    let text_wrap_mode = text_wrap_mode(itext_styles.text_wrap_mode);

    parley::TextStyle {
        font_family: parley_types::FontFamily::List(Cow::Owned(families)),
        font_size,
        font_width,
        font_style,
        font_weight,
        font_variations: parley_types::FontVariations::List(Cow::Owned(font_variations)),
        font_features: parley_types::FontFeatures::List(Cow::Owned(font_features)),
        locale: parley_types::Language::parse(&font_styles._x_lang.0).ok(),
        line_height,
        vertical_align,
        word_spacing,
        letter_spacing,
        text_wrap_mode,
        white_space_collapse: white_space_collapse(itext_styles.white_space_collapse),
        overflow_wrap,
        word_break,
        line_break,
        brush: TextBrush { id: span_id },
        has_underline: Default::default(),
        underline_offset: Default::default(),
        underline_size: Default::default(),
        underline_brush: Default::default(),
        has_strikethrough: Default::default(),
        strikethrough_offset: Default::default(),
        strikethrough_size: Default::default(),
        strikethrough_brush: Default::default(),
    }
}

// Silence unused-import warnings for Direction / UnicodeBidi kept for Blitz parity.
#[allow(dead_code)]
fn _parity_imports(_: stylo::Direction, _: stylo::UnicodeBidi) {}
