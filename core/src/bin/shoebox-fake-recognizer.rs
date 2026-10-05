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
//! One cue gives faces of one person with a chosen similarity (for people
//! and suggestions, `core/tests/people.rs`):
//!
//! | Top quarter white (all channels > 250), bottom quarter grey (channels within 4 of each other) | Reply |
//! |---|---|
//! | the middle half has mean colour *c*, the grey is *g* | one face in the middle (as for any picture), of the person a plain picture of colour *c* shows, with a cosine similarity of round(40 *g* / 255) / 40 to that plain picture's face |
//!
//! The rest of such an embedding points in a direction of its own per
//! person and grey (pseudo-random, at right angles to the person's), so two
//! such faces with similarities *a* and *b* to the person are about *a*·*b*
//! similar to each other (± 0.1), and two with the same grey are the same
//! face. So grey 147 (0.575) is close enough to suggest but not to cluster,
//! 115 (0.45) only "maybe", 64 (0.25) nothing.
//!
//! The `embed` task (protocol 2, faces drawn by hand) embeds each box from
//! the mean colour of the picture inside it, like a plain picture of that
//! colour (so a box drawn on a plain picture of a person is that person).
//! It "finds landmarks" in a box unless the inside is grey (channels within
//! 4 of each other): then the plain crop is embedded and no landmarks come
//! back. The misbehaviour cues above are only for `faces`.
//!
//! With `--animals` the hello also lists the task `animals` (model
//! `fake-animals-1`, embeddings of 64 numbers, so they can never be mixed up
//! with faces), which finds one cat or dog in the middle of every picture
//! (the animal of the picture's mean colour: a cat when red is at least blue,
//! else a dog; 64-d embedding, same colour same animal):
//!
//! | Picture | Reply to `animals` |
//! |---|---|
//! | dark (all channels < 16) | no animals |
//! | pure green | an error reply |
//! | white top quarter, grey bottom quarter | as for faces: the animal of the middle half's colour, `round(40 g / 255) / 40` similar to the plain one |
//! | anything else | one animal, box (0.25, 0.25, 0.5, 0.5) |
//!
//! `--protocol <n>` overrides the protocol in the hello; `--no-embed` leaves
//! `embed` out of the hello; `--silent` never says hello.

use std::io::{BufRead, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

const DIM: usize = 128;
/// Animal embeddings are shorter than face embeddings.
const ANIMAL_DIM: usize = 64;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--silent") {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        return;
    }
    let protocol: u32 = arg("--protocol").and_then(|p| p.parse().ok()).unwrap_or(2);
    let crash_once = arg("--crash-once");
    let mut out = std::io::stdout().lock();
    let mut tasks = json!({ "faces": { "model": "fake-1", "dim": DIM } });
    if !args.iter().any(|a| a == "--no-embed") {
        tasks["embed"] = json!({ "model": "fake-1", "dim": DIM });
    }
    let animals = args.iter().any(|a| a == "--animals");
    if animals {
        tasks["animals"] = json!({ "model": "fake-animals-1", "dim": ANIMAL_DIM });
    }
    let hello = json!({
        "hello": "shoebox-recognizer",
        "protocol": protocol,
        "version": "fake",
        "tasks": tasks,
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
        if req["tasks"].as_array().is_some_and(|t| t.iter().any(|t| t == "embed")) {
            let reply = embed(&req).unwrap_or_else(|e| json!({ "id": id, "error": e }));
            writeln!(out, "{reply}").unwrap();
            out.flush().unwrap();
            continue;
        }
        if req["tasks"].as_array().is_some_and(|t| t.iter().any(|t| t == "animals")) {
            let reply = if animals { animals_reply(&req) } else { json!({ "id": id, "error": "unknown task: animals" }) };
            writeln!(out, "{reply}").unwrap();
            out.flush().unwrap();
            continue;
        }
        let reply = match picture(&req) {
            Err(e) => json!({ "id": id, "error": e }),
            Ok((w, h, _, Some((Edge::Top, Cue::Variant { colour, grey })))) => variant_reply(&id, w, h, colour, grey),
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cue {
    Cyan,
    Yellow,
    /// White top: a face like the plain picture of `colour` (the middle
    /// half's), its similarity set by the bottom quarter's `grey`.
    Variant { colour: [f64; 3], grey: f64 },
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
    let (top, bottom) = (mean(0, 0, w, h / 4), mean(0, h - h / 4, w, h));
    let grey = (bottom[0] - bottom[1]).abs() < 4.0 && (bottom[1] - bottom[2]).abs() < 4.0;
    if top.iter().all(|&c| c > 250.0) && grey {
        let colour = mean(0, h / 4, w, h - h / 4);
        return Ok((w, h, mean(0, 0, w, h), Some((Edge::Top, Cue::Variant { colour, grey: bottom[1] }))));
    }
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

/// The `embed` task: per box, the person of the mean colour inside it, with
/// landmarks unless that colour is grey.
fn embed(req: &Value) -> Result<Value, String> {
    let data = BASE64.decode(req["image"].as_str().ok_or("no image")?).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&data).map_err(|_| "cannot decode image")?.to_rgb8();
    let (w, h) = (img.width(), img.height());
    let boxes = req["boxes"].as_array().ok_or("no boxes")?;
    let mut out = Vec::new();
    for b in boxes {
        let v: Vec<f64> = b.as_array().ok_or("a box is not a list")?.iter().filter_map(Value::as_f64).collect();
        let [x, y, bw, bh] = v[..] else { return Err("a box needs x, y, w, h".into()) };
        let x0 = (x.max(0.0) as u32).min(w - 1);
        let y0 = (y.max(0.0) as u32).min(h - 1);
        let x1 = ((x + bw).ceil() as u32).clamp(x0 + 1, w);
        let y1 = ((y + bh).ceil() as u32).clamp(y0 + 1, h);
        let mut sum = [0f64; 3];
        for yy in y0..y1 {
            for xx in x0..x1 {
                let p = img.get_pixel(xx, yy);
                for c in 0..3 {
                    sum[c] += p[c] as f64;
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f64;
        let [r, g, bl] = sum.map(|s| s / n);
        let grey = (r - g).abs() < 4.0 && (g - bl).abs() < 4.0;
        let landmarks: Vec<[f64; 2]> = if grey {
            Vec::new()
        } else {
            [(0.3, 0.35), (0.7, 0.35), (0.5, 0.55), (0.35, 0.75), (0.65, 0.75)].iter().map(|(u, v)| [x + u * bw, y + v * bh]).collect()
        };
        let bytes: Vec<u8> = person([r, g, bl]).iter().flat_map(|v| v.to_le_bytes()).collect();
        out.push(json!({ "landmarks": landmarks, "emb": BASE64.encode(bytes) }));
    }
    Ok(json!({ "id": req["id"], "width": w, "height": h, "embed": out }))
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

/// The `animals` task: one animal in the middle, described by colour.
fn animals_reply(req: &Value) -> Value {
    let id = &req["id"];
    let (w, h, mean, cue) = match picture(req) {
        Ok(p) => p,
        Err(e) => return json!({ "id": id, "error": e }),
    };
    let [r, g, b] = mean;
    let (colour, emb) = match cue {
        Some((Edge::Top, Cue::Variant { colour, grey })) => (colour, variant(colour, grey, ANIMAL_DIM)),
        _ if r < 16.0 && g < 16.0 && b < 16.0 => return json!({ "id": id, "width": w, "height": h, "animals": [] }),
        _ if pure(g, r, b) => return json!({ "id": id, "error": "fake: cannot handle green" }),
        _ => (mean, person_dim(mean, ANIMAL_DIM)),
    };
    let bytes: Vec<u8> = emb.iter().flat_map(|v| v.to_le_bytes()).collect();
    let (fw, fh) = (w as f64, h as f64);
    json!({
        "id": id, "width": w, "height": h,
        "animals": [{
            "species": if colour[0] >= colour[2] { "cat" } else { "dog" },
            "bbox": [fw / 4.0, fh / 4.0, fw / 2.0, fh / 2.0],
            "score": 0.9,
            "emb": BASE64.encode(bytes),
        }],
    })
}

/// The person a colour stands for. Quantised, so JPEG noise does not
/// change the person.
fn person_number(rgb: [f64; 3]) -> u64 {
    let q = rgb.map(|c| (c / 32.0).round());
    (q[0] + 2.0 * q[1] + 3.0 * q[2] + 1.0) as u64
}

/// The embedding of a plain picture of this colour: the person.
fn person(rgb: [f64; 3]) -> Vec<f32> {
    person_dim(rgb, DIM)
}

fn person_dim(rgb: [f64; 3], dim: usize) -> Vec<f32> {
    let k = person_number(rgb) as f64;
    normalised((0..dim).map(|i| ((i as f64 + 1.0) * k).sin() as f32).collect())
}

fn normalised(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn face_reply(id: &Value, w: u32, h: u32, rgb: [f64; 3]) -> Value {
    centred_face(id, w, h, &person(rgb))
}

/// The white-top cue: the person of `colour`, `grey` away from them.
fn variant_reply(id: &Value, w: u32, h: u32, colour: [f64; 3], grey: f64) -> Value {
    centred_face(id, w, h, &variant(colour, grey, DIM))
}

/// An embedding of `dim` numbers `grey`-steps away from the plain one of
/// `colour`.
fn variant(colour: [f64; 3], grey: f64, dim: usize) -> Vec<f32> {
    let p = person_dim(colour, dim);
    let steps = (grey / 255.0 * 40.0).round();
    let sim = (steps / 40.0).clamp(0.0, 1.0) as f32;
    // A direction of its own per person and grey (pseudo-random), at right
    // angles to the person's.
    let mut x = (person_number(colour) * 100 + steps as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let other: Vec<f32> = (0..dim)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0) as f32
        })
        .collect();
    let along: f32 = other.iter().zip(&p).map(|(a, b)| a * b).sum();
    let other = normalised(other.iter().zip(&p).map(|(o, q)| o - along * q).collect());
    let rest = (1.0 - sim * sim).max(0.0).sqrt();
    normalised(p.iter().zip(&other).map(|(q, o)| sim * q + rest * o).collect())
}

/// One face in the middle of the picture, half as wide and high.
fn centred_face(id: &Value, w: u32, h: u32, emb: &[f32]) -> Value {
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
