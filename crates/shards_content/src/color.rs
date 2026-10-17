//! Colours in content files: `#rgb`, `#rrggbb` or `#rrggbbaa`, sRGB, as a theme, a look or any
//! other kind writes them.

/// `#rgb`, `#rrggbb` or `#rrggbbaa` as sRGB bytes.
pub fn parse_color(text: &str) -> Result<[u8; 4], String> {
    let bad = || format!("`{text}` is not a colour: `#rrggbb` or `#rrggbbaa`");
    let hex = text.strip_prefix('#').ok_or_else(bad)?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
    match hex.len() {
        3 => {
            let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).map(|d| d * 17).map_err(|_| bad());
            Ok([digit(0)?, digit(1)?, digit(2)?, 255])
        }
        6 => Ok([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Ok([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => Err(bad()),
    }
}
