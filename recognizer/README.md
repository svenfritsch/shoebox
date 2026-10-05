# recognizer

Optional Python worker for face and pet (cat and dog) recognition. The Rust
core (`shoebox recognize`) starts it as a child process and speaks JSON lines
over stdin/stdout ([docs/protocol.md](../docs/protocol.md)): pixels in, face
boxes and embeddings out. It never touches the database or an original.

- `recognizer.py`: the worker (OpenCV: YuNet detection, SFace embeddings;
  with `--animals` also YOLOX for cats and dogs and an image embedder for
  each animal, on onnxruntime if installed, else on OpenCV).
- `fetch-models.sh [dir]`: download the ONNX models (checksummed) into
  `models/`; `SHOEBOX_NO_ANIMALS=1` leaves out the two animal models
  (~140 MB).
- `install.sh <library-root>`: standalone Python, OpenCV, numpy, the worker
  and the models into `<root>/.shoebox/recognizer/`, where `shoebox
  recognize` finds them. Run it on each kind of computer that will run
  recognition.
- `test_recognizer.py`: protocol tests
  (`python3 -m unittest -v recognizer/test_recognizer.py`; set
  `SHOEBOX_TEST_FACE` to a photo with a face, `SHOEBOX_TEST_ANIMAL` to a
  photo of a cat or dog, to test detection too).

Usage and decisions: [docs/phase4.md](../docs/phase4.md).

## Cats and dogs, and DINOv2

`shoebox recognize --animals` starts the worker with `--animals`. The embedder
is the first of `dinov2_small.onnx` and
`image_classification_ppresnet50_2022jan.onnx` found in `models/`
(`SHOEBOX_ANIMAL_EMBEDDER=dinov2-small|ppresnet50-2022jan` forces one,
`SHOEBOX_ANIMAL_BACKEND=onnxruntime|opencv` forces the runtime). DINOv2-small
tells individual animals apart better but is not fetched automatically (it is
on Hugging Face): download `onnx/model.onnx` from `onnx-community/dinov2-small`,
save it as `models/dinov2_small.onnx` (next to `recognizer.py`, or in
`.shoebox/recognizer/models/`), and run `shoebox recognize --animals` again;
the model id changes, so the animals are looked for again. Details and open
points: [docs/phase7.md](../docs/phase7.md).
