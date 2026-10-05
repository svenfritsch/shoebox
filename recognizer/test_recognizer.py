"""Protocol tests for recognizer.py (docs/protocol.md).

    python3 -m unittest recognizer/test_recognizer.py

Needs OpenCV, numpy and the models (recognizer/fetch-models.sh); skipped
otherwise. Set SHOEBOX_TEST_FACE to a photo with one clearly visible face to
also check detection and embeddings.
"""

import base64
import importlib.util
import json
import os
import subprocess
import sys
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


if __name__ == "__main__":
    unittest.main()
