//! A stand-in for the Python recognizer (`docs/protocol.md`), for tests and
//! for working on the core without Python. It "finds" one face in the middle
//! of every picture, with an embedding made from the picture's mean colour
//! (same colour, same person), and misbehaves on cue:
//!
//! | Mean colour of the picture (pure: one channel full, the others off) | Reply |
//! |---|---|
//! | dark (all channels < 16) | no faces |
//! | pure red | exits; with `--crash-once <file>`, only while the file does not exist (it is created before exiting) |
//! | pure green | an error reply |
//! | pure blue | never answers |
//! | pure magenta | a line of garbage, then the normal reply |
//!
//! One cue looks at where a colour is, not the mean, so a picture can hold a
//! face only when it is turned (for `shoebox recognize --rotated`):
//!
//! | Edge of the picture (its outer quarter) | Reply |
//! |---|---|
//! | pure cyan at the top | one face, off-centre: box (0.1, 0.5, 0.2, 0.3) of width and height, score 0.95 |
//! | pure cyan left, right or at the bottom | no faces |
//! | pure yellow at the top | never answers |
//! | pure yellow elsewhere | as if it were not there |
//!
//! A picture whose left quarter is cyan has no face upright, its face when
//! turned 90° clockwise, and none when turned 270°; one whose left quarter is
//! yellow is answered upright and hangs once turned.
//!
//! `--protocol <n>` overrides the protocol in the hello; `--silent` never
//! says hello.

use std::io::{BufRead, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

const DIM: usize = 128;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--silent") {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        return;
    }
    let protocol: u32 = arg("--protocol").and_then(|p| p.parse().ok()).unwrap_or(1);
    let crash_once = arg("--crash-once");
    let mut out = std::io::stdout().lock();
    let hello = json!({
        "hello": "shoebox-recognizer",
        "protocol": protocol,
        "version": "fake",
        "tasks": { "faces": { "model": "fake-1", "dim": DIM } },
    });
    writeln!(out, "{hello}").unwrap();
    out.flush().unwrap();

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            eprintln!("fake recognizer: not JSON");
            continue;
        };
        let id = req["id"].clone();
        let reply = match picture(&req) {
            Err(e) => json!({ "id": id, "error": e }),
            Ok((_, _, _, Some((Edge::Top, Cue::Yellow)))) => {
                std::thread::sleep(std::time::Duration::from_secs(3600));
                return;
            }
            Ok((w, h, _, Some((edge, Cue::Cyan)))) => {
                if edge == Edge::Top {
                    lying_face_reply(&id, w, h)
                } else {
                    json!({ "id": id, "width": w, "height": h, "faces": [] })
                }
            }
            Ok((w, h, [r, g, b], _)) => {
                if r < 16.0 && g < 16.0 && b < 16.0 {
                    json!({ "id": id, "width": w, "height": h, "faces": [] })
                } else if pure(r, g, b) {
                    match &crash_once {
                        Some(m) if std::path::Path::new(m).exists() => face_reply(&id, w, h, [r, g, b]),
                        Some(m) => {
                            std::fs::write(m, b"crashed").unwrap();
                            std::process::exit(3);
                        }
                        None => std::process::exit(3),
                    }
                } else if pure(g, r, b) {
                    json!({ "id": id, "error": "fake: cannot handle green" })
                } else if pure(b, r, g) {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                    return;
                } else {
                    if pure(r.min(b), g, g) {
                        writeln!(out, "garbage {{").unwrap();
                    }
                    face_reply(&id, w, h, [r, g, b])
                }
            }
        };
        writeln!(out, "{reply}").unwrap();
        out.flush().unwrap();
    }
}

/// Whether channel `high` is full and the others are off: only solid test
/// pictures trigger the misbehaviour, never a photo.
fn pure(high: f64, low1: f64, low2: f64) -> bool {
    high > 230.0 && low1 < 25.0 && low2 < 25.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cue {
    Cyan,
    Yellow,
}

/// Width, height, mean colour, and the edge that is pure cyan or yellow.
type Picture = (u32, u32, [f64; 3], Option<(Edge, Cue)>);

/// Size and mean colour of the picture in a request, and the edge that is
/// pure cyan or yellow, if one is.
fn picture(req: &Value) -> Result<Picture, String> {
    let data = BASE64.decode(req["image"].as_str().ok_or("no image")?).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&data).map_err(|_| "cannot decode image")?.to_rgb8();
    let (w, h) = (img.width(), img.height());
    let mean = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
        let mut sum = [0f64; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let p = img.get_pixel(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as f64;
                }
            }
        }
        sum.map(|s| s / n)
    };
    let cue = |[r, g, b]: [f64; 3]| {
        if pure(g.min(b), r, r) {
            Some(Cue::Cyan)
        } else if pure(r.min(g), b, b) {
            Some(Cue::Yellow)
        } else {
            None
        }
    };
    let edge = [
        (Edge::Top, mean(0, 0, w, h / 4)),
        (Edge::Bottom, mean(0, h - h / 4, w, h)),
        (Edge::Left, mean(0, 0, w / 4, h)),
        (Edge::Right, mean(w - w / 4, 0, w, h)),
    ]
    .into_iter()
    .find_map(|(e, m)| cue(m).map(|c| (e, c)));
    Ok((w, h, mean(0, 0, w, h), edge))
}

/// The face of the cyan cue, in the top-left part of the picture as sent.
fn lying_face_reply(id: &Value, w: u32, h: u32) -> Value {
    let mut emb = vec![0f32; DIM];
    emb[0] = 1.0;
    let bytes: Vec<u8> = emb.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (fw, fh) = (w as f64, h as f64);
    json!({
        "id": id, "width": w, "height": h,
        "faces": [{
            "bbox": [fw * 0.1, fh * 0.5, fw * 0.2, fh * 0.3],
            "score": 0.95,
            "landmarks": [[fw * 0.1, fh * 0.5], [fw * 0.3, fh * 0.5], [fw * 0.2, fh * 0.65], [fw * 0.15, fh * 0.75], [fw * 0.25, fh * 0.75]],
            "emb": BASE64.encode(bytes),
        }],
    })
}

fn face_reply(id: &Value, w: u32, h: u32, rgb: [f64; 3]) -> Value {
    // Quantised, so JPEG noise does not change the person.
    let q = rgb.map(|c| (c / 32.0).round());
    let mut emb: Vec<f32> = (0..DIM).map(|i| ((i as f64 + 1.0) * (q[0] + 2.0 * q[1] + 3.0 * q[2] + 1.0)).sin() as f32).collect();
    let norm = emb.iter().map(|v| v * v).sum::<f32>().sqrt();
    emb.iter_mut().for_each(|v| *v /= norm);
    let bytes: Vec<u8> = emb.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (fw, fh) = (w as f64, h as f64);
    json!({
        "id": id, "width": w, "height": h,
        "faces": [{
            "bbox": [fw / 4.0, fh / 4.0, fw / 2.0, fh / 2.0],
            "score": 0.99,
            "landmarks": [[fw * 0.4, fh * 0.4], [fw * 0.6, fh * 0.4], [fw * 0.5, fh * 0.5], [fw * 0.42, fh * 0.6], [fw * 0.58, fh * 0.6]],
            "emb": BASE64.encode(bytes),
        }],
    })
}
