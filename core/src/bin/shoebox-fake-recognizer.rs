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
            Ok((w, h, [r, g, b])) => {
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

/// Size and mean colour of the picture in a request.
fn picture(req: &Value) -> Result<(u32, u32, [f64; 3]), String> {
    let data = BASE64.decode(req["image"].as_str().ok_or("no image")?).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&data).map_err(|_| "cannot decode image")?.to_rgb8();
    let n = (img.width() * img.height()).max(1) as f64;
    let mut sum = [0f64; 3];
    for p in img.pixels() {
        for c in 0..3 {
            sum[c] += p[c] as f64;
        }
    }
    Ok((img.width(), img.height(), sum.map(|s| s / n)))
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
