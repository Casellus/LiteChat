//! Cross-platform unread badge drawing on RGBA buffers.

const BADGE_RED: [u8; 3] = [0xE5, 0x39, 0x35];

/// 3x5 pixel font for the badge digits.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        _ => [0b000, 0b010, 0b111, 0b010, 0b000], // '+'
    }
}

fn put(px: &mut [u8], size: u32, x: i32, y: i32, rgb: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 || a <= 0.0 {
        return;
    }
    let i = ((y as u32 * size + x as u32) * 4) as usize;
    let a = a.min(1.0);
    let base_a = px[i + 3] as f32 / 255.0;
    let out_a = a + base_a * (1.0 - a);
    for c in 0..3 {
        let src = rgb[c] as f32;
        let dst = px[i + c] as f32;
        px[i + c] = ((src * a + dst * base_a * (1.0 - a)) / out_a.max(1e-6)) as u8;
    }
    px[i + 3] = (out_a * 255.0) as u8;
}

/// Draws a red circle (anti-aliased) and, if `scale > 0`, the count in white.
pub fn draw_badge(px: &mut [u8], size: u32, cx: f32, cy: f32, r: f32, count: u32, scale: i32) {
    for y in 0..size as i32 {
        for x in 0..size as i32 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            put(px, size, x, y, BADGE_RED, r + 0.5 - d);
        }
    }
    if scale <= 0 {
        return;
    }
    let text = if count > 9 { "9+".to_string() } else { count.to_string() };
    let chars: Vec<char> = text.chars().collect();
    let w = chars.len() as i32 * 3 * scale + (chars.len() as i32 - 1) * scale;
    let h = 5 * scale;
    let mut ox = (cx - w as f32 / 2.0).round() as i32;
    let oy = (cy - h as f32 / 2.0).round() as i32;
    for c in chars {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(
                                px,
                                size,
                                ox + col * scale + dx,
                                oy + row as i32 * scale + dy,
                                [255; 3],
                                1.0,
                            );
                        }
                    }
                }
            }
        }
        ox += 4 * scale;
    }
}

/// Tray icon with a small red dot in the top-right corner when there are unread chats.
pub fn tray_icon_rgba(base: &[u8], size: u32, unread: u32) -> Vec<u8> {
    let mut px = base.to_vec();
    if unread > 0 {
        let s = size as f32;
        draw_badge(&mut px, size, s * 0.78, s * 0.22, s * 0.22, unread, 0);
    }
    px
}
