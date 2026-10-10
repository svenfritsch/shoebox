# recognizer

Optional Python worker for face and pet (cat and dog) recognition. The Rust
core (`shoebox recognize`) starts it as a child process and speaks JSON lines
over stdin/stdout ([docs/protocol.md](../docs/protocol.md)): pixels in, face
boxes and embeddings out. It never touches the database or an original.

- `recognizer.py`: the worker (OpenCV: YuNet detection, SFace embeddings;
  with `--pets` also YOLOX for cats and dogs and an image embedder for
  each pet, on onnxruntime if installed, else on OpenCV).
- `fetch-models.sh [dir]`: download the ONNX models (checksummed) into
  `models/`; `SHOEBOX_NO_PETS=1` leaves out the two pet models
  (~140 MB).
- `install.sh [--faces] [--pets] [library-root]`: standalone Python, OpenCV,
  numpy and the models of the add-ons asked for (Faces ~40 MB, Pets ~140 MB,
  independent; both are asked in a terminal when neither flag is given). The
  runtime (~200 MB) is always installed. Without a path everything goes into
  this folder (next to the shoebox program) and every drive you recognize uses
  it; with a path into `<root>/.shoebox/recognizer/`, which travels with that
  drive. `shoebox recognize` looks next to the program first and in the drive
  last. Run it on each kind of computer that will run recognition: the models
  are kept, only that computer's Python is added. People who do not use the
  terminal press **Install the selected add-ons** in the Control Panel (step 1,
  Add-ons), which runs this script.
- `install.ps1 [-Faces] [-Pets] [-Root library-root]` and `fetch-models.ps1`:
  the same for Windows 10/11 on x86-64, in PowerShell (no extra tools: it uses
  `Invoke-WebRequest`, `Get-FileHash` and the system's `tar.exe`). The Control
  Panel runs `powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1`.
  The Python archive and the models are checked against SHA-256 sums; the
  runtime goes to `runtime\windows-x86_64\` (`python.exe`). If OpenCV does not
  start, the Microsoft Visual C++ Redistributable (x64) is missing.
  `core/tests/recognize.rs` keeps the model sums of the two fetch scripts equal.
- `test_recognizer.py`: protocol tests
  (`python3 -m unittest -v recognizer/test_recognizer.py`; set
  `SHOEBOX_TEST_FACE` to a photo with a face, `SHOEBOX_TEST_PET` to a
  photo of a cat or dog, to test detection too).

Usage and decisions: [docs/phase4.md](../docs/phase4.md).

## Cats and dogs, and DINOv2

`shoebox recognize --pets` starts the worker with `--pets`. The embedder
is the first of `dinov2_small.onnx` and
`image_classification_ppresnet50_2022jan.onnx` found in `models/`
(`SHOEBOX_PET_EMBEDDER=dinov2-small|ppresnet50-2022jan` forces one,
`SHOEBOX_PET_BACKEND=onnxruntime|opencv` forces the runtime). DINOv2-small
tells individual pets apart better but is not fetched automatically (it is
on Hugging Face): download `onnx/model.onnx` from `onnx-community/dinov2-small`,
save it as `models/dinov2_small.onnx` (next to `recognizer.py`, or in
`.shoebox/recognizer/models/`), and run `shoebox recognize --pets` again;
the model id changes, so the pets are looked for again. Details and open
points: [docs/phase7.md](../docs/phase7.md).
