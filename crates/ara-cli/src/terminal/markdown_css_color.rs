//! CSS Color 4 parsing for the fixed OMP Bun.color consumer.
//! Parsing is delegated to cssparser-color; no independent CSS grammar is introduced.
//! Conversion and gamut mapping are adapted from Bun 1.4.0 (MIT), commit
//! 34cbb9a40b4bd1bd767d134a7065e66c2432a676, src/css/values/color.rs.
//! Bun's equations cite the CSS Color 4 sample code. Reference bytes, license,
//! and SHA are recorded in CSS-COLOR-REFERENCE-v1.json.
use cssparser::{Parser, ParserInput};
use cssparser_color::{Color, hsl_to_rgb, hwb_to_rgb};

fn matrix(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn xyz_to_rgb(xyz: [f32; 3]) -> [f32; 3] {
    matrix(
        [
            [3.2409699419045226, -1.537383177570094, -0.4986107602930034],
            [-0.9692436362808796, 1.8759675015077202, 0.04155505740717559],
            [0.05563007969699366, -0.20397695888897652, 1.0569715142428786],
        ],
        xyz,
    )
    .map(|v| {
        let gamma = if v.abs() > 0.0031308 { v.signum() * (1.055 * v.abs().powf(1. / 2.4) - 0.055) } else { 12.92 * v };
        gamma
    })
}

fn lab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    const KAPPA: f32 = (24389.0f64 / 27.0f64) as f32;
    const EPSILON: f32 = (216.0f64 / 24389.0f64) as f32;
    let fy = (l + 16.) / 116.;
    let inv = |f: f32| if f.powf(3.) > EPSILON { f.powf(3.) } else { (116. * f - 16.) / KAPPA };
    let xyz = [
        inv(a / 500. + fy) * (0.3457f64 / 0.3585f64) as f32,
        if l > KAPPA * EPSILON { fy.powf(3.) } else { l / KAPPA },
        inv(fy - b / 200.) * ((1.0f64 - 0.3457f64 - 0.3585f64) / 0.3585f64) as f32,
    ];
    xyz_to_rgb(matrix(
        [
            [0.9554734527042182, -0.023098536874261423, 0.0632593086610217],
            [-0.028369706963208136, 1.0099954580058226, 0.021041398966943008],
            [0.012314001688319899, -0.020507696433477912, 1.3303659366080753],
        ],
        xyz,
    ))
}

fn oklab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let lms = matrix(
        [
            [0.99999999845051981432, 0.39633779217376785678, 0.21580375806075880339],
            [1.0000000088817607767, -0.1055613423236563494, -0.063854174771705903402],
            [1.0000000546724109177, -0.089484182094965759684, -1.2914855378640917399],
        ],
        [l, a, b],
    )
    .map(|v| v.powf(3.));
    xyz_to_rgb(matrix(
        [
            [1.2268798733741557, -0.5578149965554813, 0.28139105017721583],
            [-0.04057576262431372, 1.1122868293970594, -0.07171106666151701],
            [-0.07637294974672142, -0.4214933239627914, 1.5869240244272418],
        ],
        lms,
    ))
}

fn polar(c: Option<f32>, h: Option<f32>) -> (f32, f32) {
    let c = c.unwrap_or(0.).max(0.);
    let h = h.unwrap_or(0.) * std::f32::consts::PI / 180.;
    (c * h.cos(), c * h.sin())
}

fn rgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let lin =
        rgb.map(|v| if v.abs() < 0.04045 { v / 12.92 } else { v.signum() * ((v.abs() + 0.055) / 1.055).powf(2.4) });
    let xyz = matrix(
        [
            [0.41239079926595934, 0.357584339383878, 0.1804807884018343],
            [0.21263900587151027, 0.715168678767756, 0.07219231536073371],
            [0.01933081871559182, 0.11919477979462598, 0.9505321522496607],
        ],
        lin,
    );
    let lms = matrix(
        [
            [0.8190224432164319, 0.3619062562801221, -0.12887378261216414],
            [0.0329836671980271, 0.9292868468965546, 0.03614466816999844],
            [0.048177199566046255, 0.26423952494422764, 0.6335478258136937],
        ],
        xyz,
    )
    .map(f32::cbrt);
    matrix(
        [
            [0.2104542553, 0.793617785, -0.0040720468],
            [1.9779984951, -2.428592205, 0.4505937099],
            [0.0259040371, 0.7827717662, -0.808675766],
        ],
        lms,
    )
}

fn map_gamut(rgb: [f32; 3]) -> [f32; 3] {
    if rgb.iter().all(|v| (0. ..=1.).contains(v)) {
        return rgb;
    }
    const JND: f32 = 0.02;
    const EPSILON: f32 = 0.00001;
    let [l, a, b] = rgb_to_oklab(rgb);
    if (l - 1.).abs() < EPSILON || l > 1. {
        return oklab_to_rgb(1., 0., 0.);
    }
    if l < EPSILON {
        return oklab_to_rgb(0., 0., 0.);
    }
    let mut chroma = (a * a + b * b).sqrt();
    let mut hue = b.atan2(a) * 180. / std::f32::consts::PI;
    if hue < 0. {
        hue += 360.;
    }
    hue = hue.rem_euclid(360.);
    let angle = hue * std::f32::consts::PI / 180.;
    let convert = |c| oklab_to_rgb(l, c * angle.cos(), c * angle.sin());
    let delta = |clipped: [f32; 3], c| {
        let actual = rgb_to_oklab(clipped);
        let desired = [l, c * angle.cos(), c * angle.sin()];
        ((actual[0] - desired[0]).powi(2) + (actual[1] - desired[1]).powi(2) + (actual[2] - desired[2]).powi(2)).sqrt()
    };
    let clipped = convert(chroma).map(|v| v.clamp(0., 1.));
    if delta(clipped, chroma) < JND {
        return clipped;
    }
    let mut min = 0.;
    let mut max = chroma;
    while max - min > EPSILON {
        chroma = (min + max) / 2.;
        let converted = convert(chroma);
        if converted.iter().all(|v| (0. ..=1.).contains(v)) {
            min = chroma;
            continue;
        }
        let clipped = converted.map(|v| v.clamp(0., 1.));
        if delta(clipped, chroma) < JND {
            return clipped;
        }
        max = chroma;
    }
    convert(chroma)
}

pub(super) fn parse_rgb(spec: &str) -> Option<[f64; 3]> {
    let mut input = ParserInput::new(spec);
    let mut parser = Parser::new(&mut input);
    let color = parser.parse_entirely(Color::parse).ok()?;
    let f = |v: Option<f32>| v.unwrap_or(0.);
    let rgb = match color {
        Color::Rgba(v) => return Some([v.red as f64, v.green as f64, v.blue as f64]),
        Color::Hsl(v) => {
            let (r, g, b) =
                hsl_to_rgb(v.hue.unwrap_or(0.) / 360., v.saturation.unwrap_or(0.), v.lightness.unwrap_or(0.));
            [r, g, b]
        }
        Color::Hwb(v) => {
            let (r, g, b) =
                hwb_to_rgb(v.hue.unwrap_or(0.) / 360., v.whiteness.unwrap_or(0.), v.blackness.unwrap_or(0.));
            [r, g, b]
        }
        Color::Lab(v) => lab_to_rgb(f(v.lightness).max(0.), f(v.a), f(v.b)),
        Color::Lch(v) => {
            let (a, b) = polar(v.chroma, v.hue);
            lab_to_rgb(f(v.lightness).max(0.), a, b)
        }
        Color::Oklab(v) => oklab_to_rgb(f(v.lightness).max(0.), f(v.a), f(v.b)),
        Color::Oklch(v) => {
            let (a, b) = polar(v.chroma, v.hue);
            oklab_to_rgb(f(v.lightness).max(0.), a, b)
        }
        // Fixed Bun returns the original string for these values. OMP's ansiColor
        // then rejects it because it does not begin with the ANSI foreground prefix.
        Color::CurrentColor | Color::ColorFunction(_) => return None,
    };
    rgb.iter().all(|v| v.is_finite()).then(|| map_gamut(rgb).map(|v| (v * 255.).round().clamp(0., 255.) as f64))
}

/// Exact Bun 1.4.0 color_js.rs / tmux quantization, including grey-vs-cube distance.
pub(super) fn ansi256(rgb: [u8; 3]) -> u16 {
    const Q2C: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let values = rgb.map(i32::from);
    let cube = values.map(|v| {
        if v < 48 {
            0
        } else if v < 114 {
            1
        } else {
            (v - 35) / 40
        }
    });
    let palette = cube.map(|v| Q2C[v as usize]);
    let index = (16 + 36 * cube[0] + 6 * cube[1] + cube[2]) as u16;
    if palette == values {
        return index;
    }
    let avg = (values[0] + values[1] + values[2]) / 3;
    let grey_idx = if avg > 238 { 23 } else { (avg - 3).max(0) / 10 };
    let grey = 8 + 10 * grey_idx;
    let distance = |v: [i32; 3]| v.iter().zip(values).map(|(a, b)| (a - b) * (a - b)).sum::<i32>();
    if distance([grey; 3]) < distance(palette) { (232 + grey_idx) as u16 } else { index }
}
