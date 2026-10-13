//! Minified `weather_clothing` source reformats to the canonical layout below.

use lemma::format_source;
use lemma::SourceType;

const WEATHER_CLOTHING_MINIFIED: &str = r##"spec weather_clothing """ A commentary line that exceeds MAX_COLS but doesn't get wrapped. """ data temperature: measure -> unit celsius: 1.0 -> minimum -70 celsius -> maximum 70 celsius data wind_speed: number -> minimum 0 data is_raining: boolean rule clothing_layer: "light" unless temperature < 10 celsius then "warm" unless temperature < 5 celsius then "very_warm" unless temperature > 25 celsius then "none" rule needs_jacket: no unless temperature < 15 celsius then yes unless is_raining  then yes unless wind_speed > 20 then yes rule needs_umbrella: is_raining rule needs_hat: no unless temperature > 25 celsius then yes unless temperature < 0 celsius then yes rule comfort_level: "comfortable" unless temperature < 5 celsius then "cold" unless temperature > 30 celsius then "hot" unless is_raining and temperature < 10 celsius then "uncomfortable" rule recommendation: "Enjoy your day! You don't need a hat." unless comfort_level is "cold" then "Dress warmly and stay indoors if possible" unless comfort_level is "hot" then "Stay hydrated and seek shade" unless comfort_level is "uncomfortable" then "Consider postponing outdoor activities""##;

const WEATHER_CLOTHING_FORMATTED_EXPECTED: &str = r##"spec weather_clothing
"""
A commentary line that exceeds MAX_COLS but doesn't get wrapped.
"""

data temperature: measure
  -> unit celsius: 1.0
  -> minimum -70 celsius
  -> maximum 70 celsius

data wind_speed: number
  -> minimum 0

data is_raining: boolean

rule clothing_layer:
  "light"
  unless temperature < 10 celsius then "warm"
  unless temperature < 5 celsius  then "very_warm"
  unless temperature > 25 celsius then "none"

rule needs_jacket:
  no
  unless temperature < 15 celsius then yes
  unless is_raining               then yes
  unless wind_speed > 20          then yes

rule needs_umbrella:
  is_raining

rule needs_hat:
  no
  unless temperature > 25 celsius then yes
  unless temperature < 0 celsius  then yes

rule comfort_level:
  "comfortable"
  unless temperature < 5 celsius
    then "cold"
  unless temperature > 30 celsius
    then "hot"
  unless is_raining and temperature < 10 celsius
    then "uncomfortable"

rule recommendation:
  "Enjoy your day! You don't need a hat."
  unless comfort_level is "cold"
    then "Dress warmly and stay indoors if possible"
  unless comfort_level is "hot"
    then "Stay hydrated and seek shade"
  unless comfort_level is "uncomfortable"
    then "Consider postponing outdoor activities"
"##;

#[test]
fn weather_clothing_minified_formats_expected() {
    let formatted = format_source(WEATHER_CLOTHING_MINIFIED, SourceType::Volatile).unwrap();
    assert_eq!(formatted, WEATHER_CLOTHING_FORMATTED_EXPECTED);
}

#[test]
fn weather_clothing_format_idempotent() {
    let once = format_source(WEATHER_CLOTHING_MINIFIED, SourceType::Volatile).unwrap();
    let twice = format_source(&once, SourceType::Volatile).unwrap();
    assert_eq!(once, twice);
}
