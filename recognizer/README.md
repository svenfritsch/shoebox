# recognizer

Optional Python worker for face recognition (pet recognition later). The Rust
core (`shoebox recognize`) starts it as a child process and speaks JSON lines
over stdin/stdout ([docs/protocol.md](../docs/protocol.md)): pixels in, face
boxes and embeddings out. It never touches the database or an original.

- `recognizer.py`: the worker (OpenCV: YuNet detection, SFace embeddings).
- `fetch-models.sh [dir]`: download the ONNX models (checksummed) into
  `models/`.
- `install.sh <library-root>`: standalone Python, OpenCV, numpy, the worker
  and the models into `<root>/.shoebox/recognizer/`, where `shoebox
  recognize` finds them. Run it on each kind of computer that will run
  recognition.
- `test_recognizer.py`: protocol tests
  (`python3 -m unittest -v recognizer/test_recognizer.py`; set
  `SHOEBOX_TEST_FACE` to a photo with a face to test detection too).

Usage and decisions: [docs/phase4.md](../docs/phase4.md).
