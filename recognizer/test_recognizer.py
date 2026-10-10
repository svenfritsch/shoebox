"""Protocol tests for recognizer.py (docs/protocol.md).

    python3 -m unittest recognizer/test_recognizer.py

Needs OpenCV, numpy and the models (recognizer/fetch-models.sh); skipped
otherwise. Set SHOEBOX_TEST_FACE to a photo with one clearly visible face to
also check detection and embeddings, and SHOEBOX_TEST_PET to a photo of a
cat or dog to check the pets task. The embedder tests need the `onnx`
package (pip install onnx) and run with either runtime.
"""

import base64
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
# Loaded by path: run from the repo root, `recognizer` is this folder.
_spec = importlib.util.spec_from_file_location("shoebox_recognizer", os.path.join(HERE, "recognizer.py"))
recognizer = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(recognizer)

try:
    import cv2
    import numpy as np
except ImportError:
    cv2 = None

MODELS = recognizer.models_dir()
HAVE_MODELS = all(os.path.isfile(os.path.join(MODELS, m)) for m in (recognizer.DETECTOR, recognizer.EMBEDDER))


HAVE_PET_MODELS = HAVE_MODELS and os.path.isfile(os.path.join(MODELS, recognizer.PET_DETECTOR)) and any(
    os.path.isfile(os.path.join(MODELS, s["file"])) for s in recognizer.EMBEDDERS
)

try:
    import onnx
    from onnx import TensorProto, helper
except ImportError:
    onnx = None

try:
    import rapidocr_onnxruntime  # noqa: F401
    HAVE_RAPIDOCR = True
except ImportError:
    HAVE_RAPIDOCR = False
HAVE_TEXT_MODELS = all(os.path.isfile(os.path.join(MODELS, f)) for f in recognizer.TEXT_FILES)


@unittest.skipUnless(cv2 is not None and HAVE_MODELS, "needs opencv, numpy and recognizer/fetch-models.sh")
class Protocol(unittest.TestCase):
    def setUp(self):
        self.proc = subprocess.Popen(
            [sys.executable, os.path.join(HERE, "recognizer.py")],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        self.hello = json.loads(self.proc.stdout.readline())

    def tearDown(self):
        self.proc.stdin.close()
        self.assertEqual(self.proc.wait(timeout=10), 0)
        self.proc.stdout.close()

    def ask(self, req):
        self.proc.stdin.write(json.dumps(req) + "\n")
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def jpeg(self, img):
        ok, data = cv2.imencode(".jpg", img)
        self.assertTrue(ok)
        return base64.b64encode(data.tobytes()).decode()

    def test_hello(self):
        self.assertEqual(self.hello["hello"], "shoebox-recognizer")
        self.assertEqual(self.hello["protocol"], 2)
        self.assertEqual(self.hello["tasks"]["faces"]["dim"], 128)
        self.assertEqual(self.hello["tasks"]["embed"], self.hello["tasks"]["faces"])

    def test_blank_picture_has_no_faces(self):
        img = np.full((480, 640, 3), 200, np.uint8)
        reply = self.ask({"id": 7, "tasks": ["faces"], "image": self.jpeg(img)})
        self.assertEqual(reply, {"id": 7, "width": 640, "height": 480, "faces": []})

    def test_errors_keep_the_worker_running(self):
        self.proc.stdin.write("not json\n")
        self.assertEqual(self.ask({"id": 1, "tasks": ["faces"], "image": "AAAA"}), {"id": 1, "error": "cannot decode image"})
        self.assertIn("error", self.ask({"id": 2, "tasks": ["faces"], "path": "/does/not/exist.jpg"}))
        self.assertIn("unknown task", self.ask({"id": 3, "tasks": ["cats"], "image": self.jpeg(np.zeros((8, 8, 3), np.uint8))})["error"])
        self.assertEqual(self.ask({"id": 4, "tasks": [], "image": self.jpeg(np.zeros((8, 8, 3), np.uint8))})["id"], 4)

    def test_embed_without_landmarks_embeds_the_plain_crop(self):
        img = np.full((480, 640, 3), 200, np.uint8)
        reply = self.ask({"id": 8, "tasks": ["embed"], "image": self.jpeg(img), "boxes": [[100, 100, 80, 90], [0, 0, 640, 480]]})
        self.assertEqual((reply["width"], reply["height"]), (640, 480))
        self.assertEqual(len(reply["embed"]), 2)
        for e in reply["embed"]:
            self.assertEqual(e["landmarks"], [])
            emb = np.frombuffer(base64.b64decode(e["emb"]), "<f4")
            self.assertEqual(emb.shape, (128,))
            self.assertAlmostEqual(float(np.linalg.norm(emb)), 1.0, places=4)
        self.assertIn("error", self.ask({"id": 9, "tasks": ["embed"], "image": self.jpeg(img)}))
        self.assertIn("error", self.ask({"id": 10, "tasks": ["embed"], "image": self.jpeg(img), "boxes": [[700, 10, 20, 20]]}))

    def test_large_pictures_are_shrunk(self):
        reply = self.ask({"id": 5, "tasks": [], "image": self.jpeg(np.zeros((1000, 3200, 3), np.uint8))})
        self.assertEqual((reply["width"], reply["height"]), (1600, 500))

    @unittest.skipUnless(os.environ.get("SHOEBOX_TEST_FACE"), "set SHOEBOX_TEST_FACE to a photo with a face")
    def test_face(self):
        img = cv2.imread(os.environ["SHOEBOX_TEST_FACE"])
        a = self.ask({"id": 1, "tasks": ["faces"], "image": self.jpeg(img)})["faces"]
        self.assertGreaterEqual(len(a), 1)
        face = a[0]
        self.assertEqual(len(face["landmarks"]), 5)
        emb = np.frombuffer(base64.b64decode(face["emb"]), "<f4")
        self.assertEqual(emb.shape, (128,))
        self.assertAlmostEqual(float(np.linalg.norm(emb)), 1.0, places=4)

        # The same face mirrored and at another size is still the same person.
        small = cv2.resize(cv2.flip(img, 1), None, fx=0.8, fy=0.8, interpolation=cv2.INTER_AREA)
        b = self.ask({"id": 2, "tasks": ["faces"], "image": self.jpeg(small)})["faces"]
        best = max(float(np.dot(emb, np.frombuffer(base64.b64decode(f["emb"]), "<f4"))) for f in b)
        self.assertGreater(best, 0.5)

        # Drawn by hand around the detected face (a little off): landmarks
        # are found and the embedding is the detected face's.
        x, y, w, h = face["bbox"]
        drawn = [x - 0.1 * w, y + 0.05 * h, 1.1 * w, 1.05 * h]
        e = self.ask({"id": 3, "tasks": ["embed"], "image": self.jpeg(img), "boxes": [drawn]})["embed"][0]
        self.assertEqual(len(e["landmarks"]), 5)
        same = float(np.dot(emb, np.frombuffer(base64.b64decode(e["emb"]), "<f4")))
        self.assertGreater(same, 0.8)


@unittest.skipUnless(cv2 is not None and HAVE_PET_MODELS, "needs opencv, numpy and the pet models")
class Pets(unittest.TestCase):
    """The task `pets`, only there when the worker is started with --pets."""

    def start(self, *args, env=None):
        proc = subprocess.Popen(
            [sys.executable, os.path.join(HERE, "recognizer.py"), *args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=dict(os.environ, **(env or {})),
        )
        self.addCleanup(lambda: (proc.stdin.close(), proc.wait(timeout=10), proc.stdout.close()))
        return proc, json.loads(proc.stdout.readline())

    def ask(self, proc, req):
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        return json.loads(proc.stdout.readline())

    def jpeg(self, img):
        ok, data = cv2.imencode(".jpg", img)
        self.assertTrue(ok)
        return base64.b64encode(data.tobytes()).decode()

    def test_pets_are_loaded_only_when_asked_for(self):
        _, hello = self.start()
        self.assertNotIn("pets", hello["tasks"])
        proc, hello = self.start("--pets")
        info = hello["tasks"]["pets"]
        self.assertTrue(info["model"].startswith("yolox-s-2022nov+"), info)
        self.assertGreater(info["dim"], 0)
        # Faces keep their own model: the two never mix.
        self.assertEqual(hello["tasks"]["faces"]["dim"], 128)
        self.assertNotEqual(info["model"], hello["tasks"]["faces"]["model"])
        reply = self.ask(proc, {"id": 1, "tasks": ["pets", "faces"], "image": self.jpeg(np.full((480, 640, 3), 200, np.uint8))})
        self.assertEqual(reply, {"id": 1, "width": 640, "height": 480, "pets": [], "faces": []})

    def test_pets_without_the_flag_is_an_unknown_task(self):
        proc, _ = self.start()
        reply = self.ask(proc, {"id": 1, "tasks": ["pets"], "image": self.jpeg(np.zeros((8, 8, 3), np.uint8))})
        self.assertIn("unknown task", reply["error"])

    def test_pets_drawn_by_hand_are_embedded_like_detected_ones(self):
        proc, hello = self.start("--pets")
        info = hello["tasks"]["pets"]
        self.assertEqual(hello["tasks"]["embed-pets"], info)
        img = np.full((480, 640, 3), 200, np.uint8)
        reply = self.ask(proc, {"id": 1, "tasks": ["embed-pets"], "image": self.jpeg(img), "boxes": [[100, 100, 200, 150], [0, 0, 640, 480]]})
        self.assertEqual((reply["width"], reply["height"]), (640, 480))
        self.assertEqual(len(reply["embed-pets"]), 2)
        for e in reply["embed-pets"]:
            emb = np.frombuffer(base64.b64decode(e["emb"]), "<f4")
            self.assertEqual(emb.shape, (info["dim"],))
            self.assertAlmostEqual(float(np.linalg.norm(emb)), 1.0, places=4)
        self.assertIn("error", self.ask(proc, {"id": 2, "tasks": ["embed-pets"], "image": self.jpeg(img)}))
        self.assertIn("error", self.ask(proc, {"id": 3, "tasks": ["embed-pets"], "image": self.jpeg(img), "boxes": [[700, 10, 20, 20]]}))
        self.assertIn("error", self.ask(proc, {"id": 4, "tasks": ["embed-pets"], "image": self.jpeg(img), "boxes": [[1, 2, 3]]}))
        # Without the flag the task does not exist.
        plain_proc, plain_hello = self.start()
        self.assertNotIn("embed-pets", plain_hello["tasks"])

    @unittest.skipUnless(os.environ.get("SHOEBOX_TEST_PET"), "set SHOEBOX_TEST_PET to a photo of a cat or dog")
    def test_a_drawn_box_around_a_detected_pet_matches_its_embedding(self):
        img = cv2.imread(os.environ["SHOEBOX_TEST_PET"])
        proc, _ = self.start("--pets")
        found = self.ask(proc, {"id": 1, "tasks": ["pets"], "image": self.jpeg(img)})["pets"][0]
        emb = np.frombuffer(base64.b64decode(found["emb"]), "<f4")
        x, y, w, h = found["bbox"]
        # Drawn a little off, as a finger would.
        drawn = [x + 0.03 * w, y + 0.02 * h, 0.95 * w, 0.96 * h]
        e = self.ask(proc, {"id": 2, "tasks": ["embed-pets"], "image": self.jpeg(img), "boxes": [drawn]})["embed-pets"][0]
        same = float(np.dot(emb, np.frombuffer(base64.b64decode(e["emb"]), "<f4")))
        self.assertGreater(same, 0.95)

    def test_the_embedder_can_be_chosen(self):
        for spec in recognizer.EMBEDDERS:
            if os.path.isfile(os.path.join(MODELS, spec["file"])):
                _, hello = self.start("--pets", env={"SHOEBOX_PET_EMBEDDER": spec["id"]})
                self.assertTrue(hello["tasks"]["pets"]["model"].endswith("+" + spec["id"]))

    @unittest.skipUnless(os.environ.get("SHOEBOX_TEST_PET"), "set SHOEBOX_TEST_PET to a photo of a cat or dog")
    def test_pet(self):
        img = cv2.imread(os.environ["SHOEBOX_TEST_PET"])
        for backend in ("onnxruntime", "opencv"):
            if backend == "onnxruntime":
                try:
                    import onnxruntime  # noqa: F401
                except ImportError:
                    continue
            proc, hello = self.start("--pets", env={"SHOEBOX_PET_BACKEND": backend})
            dim = hello["tasks"]["pets"]["dim"]
            found = self.ask(proc, {"id": 1, "tasks": ["pets"], "image": self.jpeg(img)})["pets"]
            self.assertGreaterEqual(len(found), 1, backend)
            a = found[0]
            self.assertIn(a["species"], ("cat", "dog"))
            x, y, w, h = a["bbox"]
            self.assertTrue(w > 0 and h > 0 and x >= 0 and y >= 0, a)
            emb = np.frombuffer(base64.b64decode(a["emb"]), "<f4")
            self.assertEqual(emb.shape, (dim,))
            self.assertAlmostEqual(float(np.linalg.norm(emb)), 1.0, places=4)

            # The same pet mirrored and smaller is still found, and its
            # embedding is close.
            small = cv2.resize(cv2.flip(img, 1), None, fx=0.7, fy=0.7, interpolation=cv2.INTER_AREA)
            again = self.ask(proc, {"id": 2, "tasks": ["pets"], "image": self.jpeg(small)})["pets"]
            best = max(float(np.dot(emb, np.frombuffer(base64.b64decode(f["emb"]), "<f4"))) for f in again)
            self.assertGreater(best, 0.9, backend)


@unittest.skipUnless(cv2 is not None and onnx is not None, "needs opencv, numpy and onnx")
class EmbedderPlumbing(unittest.TestCase):
    """The embedder reads the right output of a model shaped like Hugging
    Face's DINOv2 export, on either runtime. (The real DINOv2 file cannot be
    checked into the repository; this checks the code around it.)"""

    def model(self):
        import numpy
        w = numpy.arange(24, dtype=numpy.float32).reshape(3, 8) / 24.0 + 0.1
        pooled = helper.make_node("GlobalAveragePool", ["pixel_values"], ["pool4"])
        flat = helper.make_node("Flatten", ["pool4"], ["flat"], axis=1)
        mm = helper.make_node("MatMul", ["flat", "w"], ["cls"])
        # Tokens: the class token and a different second one.
        two = helper.make_node("Concat", ["cls3", "other3"], ["last_hidden_state"], axis=1)
        un = helper.make_node("Unsqueeze", ["cls", "axes"], ["cls3"])
        neg = helper.make_node("Neg", ["cls"], ["negcls"])
        un2 = helper.make_node("Unsqueeze", ["negcls", "axes"], ["other3"])
        ident = helper.make_node("Identity", ["cls"], ["pooler_output"])
        graph = helper.make_graph(
            [pooled, flat, mm, un, neg, un2, two, ident],
            "fake_dinov2",
            [helper.make_tensor_value_info("pixel_values", TensorProto.FLOAT, [1, 3, 224, 224])],
            [
                helper.make_tensor_value_info("last_hidden_state", TensorProto.FLOAT, [1, 2, 8]),
                helper.make_tensor_value_info("pooler_output", TensorProto.FLOAT, [1, 8]),
            ],
            initializer=[
                helper.make_tensor("w", TensorProto.FLOAT, [3, 8], w.flatten().tolist()),
                helper.make_tensor("axes", TensorProto.INT64, [1], [1]),
            ],
        )
        m = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
        m.ir_version = 8
        return m

    def test_outputs_and_backends(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "fake.onnx")
            onnx.save(self.model(), path)
            backends = ["opencv"]
            try:
                import onnxruntime  # noqa: F401
                backends.append("onnxruntime")
            except ImportError:
                pass
            seen = []
            for backend in backends:
                os.environ["SHOEBOX_PET_BACKEND"] = backend
                try:
                    for output in ("pooler_output", "last_hidden_state", None):
                        spec = {"id": "fake", "file": "fake.onnx", "size": 224, "mean": (0, 0, 0), "std": (1, 1, 1), "output": output}
                        e = recognizer.Embedder(cv2, np, spec, path)
                        self.assertEqual(e.dim, 8, (backend, output))
                        self.assertEqual(e.backend, backend)
                        square = np.full((224, 224, 3), 120, np.uint8)
                        v = np.frombuffer(base64.b64decode(e(square)), "<f4")
                        self.assertAlmostEqual(float(np.linalg.norm(v)), 1.0, places=4)
                        seen.append((backend, output, v))
                finally:
                    os.environ.pop("SHOEBOX_PET_BACKEND")
            # The class token (first token) of the 3-D output is the pooler
            # output; both runtimes agree.
            first = seen[0][2]
            for backend, output, v in seen:
                self.assertTrue(np.allclose(v, first, atol=1e-4), (backend, output))


@unittest.skipUnless(cv2 is not None, "needs opencv and numpy")
class TextPlumbing(unittest.TestCase):
    """The task `text` without models: what the worker makes of an engine's result."""

    def run_text(self, result, shape=(100, 200, 3)):
        text = recognizer.Text(cv2, np, engine=lambda img: (result, [0, 0, 0]))
        return text(np.zeros(shape, np.uint8), {})

    def test_lines_are_boxes_sorted_top_to_bottom(self):
        quad = lambda x0, y0, x1, y1: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        lines = self.run_text([
            [quad(10, 60, 90, 80), "second", "0.9"],
            [quad(12, 10, 150, 30), "first", 0.95],
        ])
        self.assertEqual([l["text"] for l in lines], ["first", "second"])
        self.assertEqual(lines[0]["bbox"], [12.0, 10.0, 138.0, 20.0])
        self.assertEqual(lines[1]["score"], 0.9)

    def test_nothing_found_is_an_empty_list(self):
        self.assertEqual(self.run_text(None), [])
        self.assertEqual(self.run_text([]), [])

    def test_boxes_are_clamped_and_empty_lines_dropped(self):
        quad = lambda x0, y0, x1, y1: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        lines = self.run_text([
            [quad(-5, -5, 50, 20), "edge", 0.8],
            [quad(10, 10, 60, 30), "   ", 0.9],
            [quad(300, 10, 400, 30), "outside", 0.9],
        ])
        self.assertEqual([l["text"] for l in lines], ["edge"])
        self.assertEqual(lines[0]["bbox"], [0.0, 0.0, 50.0, 20.0])

    def test_tilted_text_gets_its_enclosing_box(self):
        lines = self.run_text([[[[20, 40], [100, 20], [105, 40], [25, 60]], "tilted", 0.9]])
        self.assertEqual(lines[0]["bbox"], [20.0, 20.0, 85.0, 40.0])


@unittest.skipUnless(cv2 is not None and HAVE_RAPIDOCR and HAVE_TEXT_MODELS,
                     "needs opencv, rapidocr-onnxruntime and the text models in the models folder")
class TextTask(unittest.TestCase):
    """The task `text`, only there when the worker is started with --text."""

    def start(self, *args):
        proc = subprocess.Popen(
            [sys.executable, os.path.join(HERE, "recognizer.py"), *args],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        self.addCleanup(lambda: (proc.stdin.close(), proc.wait(timeout=10), proc.stdout.close()))
        return proc, json.loads(proc.stdout.readline())

    def ask(self, proc, req):
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        return json.loads(proc.stdout.readline())

    def jpeg(self, img):
        ok, data = cv2.imencode(".jpg", img)
        self.assertTrue(ok)
        return base64.b64encode(data.tobytes()).decode()

    def test_text_is_loaded_only_when_asked_for(self):
        if HAVE_MODELS:  # a worker without flags is the faces worker, which needs the face models
            _, hello = self.start()
            self.assertNotIn("text", hello["tasks"])
        proc, hello = self.start("--text")
        self.assertEqual(hello["tasks"]["text"], {"model": recognizer.TEXT_MODEL, "dim": 0})
        # With the text models alone the worker does not pretend to do faces.
        if not HAVE_MODELS:
            self.assertNotIn("faces", hello["tasks"])
        reply = self.ask(proc, {"id": 1, "tasks": ["text"], "image": self.jpeg(np.full((240, 320, 3), 255, np.uint8))})
        self.assertEqual(reply, {"id": 1, "width": 320, "height": 240, "text": []})

    def test_text_without_the_flag_is_an_unknown_task(self):
        if not HAVE_MODELS:
            self.skipTest("a worker without flags needs the face models")
        proc, _ = self.start()
        reply = self.ask(proc, {"id": 1, "tasks": ["text"], "image": self.jpeg(np.zeros((8, 8, 3), np.uint8))})
        self.assertIn("unknown task", reply["error"])

    def test_a_printed_word_is_read(self):
        img = np.full((200, 700, 3), 255, np.uint8)
        cv2.putText(img, "Rechnung 2024", (30, 120), cv2.FONT_HERSHEY_SIMPLEX, 2.2, (20, 20, 20), 4, cv2.LINE_AA)
        proc, _ = self.start("--text")
        reply = self.ask(proc, {"id": 2, "tasks": ["text"], "image": self.jpeg(img)})
        lines = reply["text"]
        self.assertTrue(lines, reply)
        self.assertIn("rechnung", " ".join(l["text"] for l in lines).lower())
        x, y, w, h = lines[0]["bbox"]
        self.assertTrue(0 <= x < 700 and 0 <= y < 200 and w > 100 and h > 20, lines[0])
        self.assertGreater(lines[0]["score"], 0.5)


if __name__ == "__main__":
    unittest.main()
