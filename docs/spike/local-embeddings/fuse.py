"""Milestone 4, Task 10 (docs/spike/local-embeddings.md): bge-m3's graph with its attention fused.

BAAI's ONNX export computes attention with separate MatMul and Softmax nodes, so ONNX Runtime
holds each layer's full score matrices: a text of n tokens needs about 1.84 GB + 136 bytes x n^2
(7.9 GB at 6,706 tokens). This script fuses the 24 attention blocks into ONNX Runtime's
`MultiHeadAttention` with no mask (one text at a time, so no padding), which runs as
FlashAttention on the CPU and keeps memory linear in n. The vectors do not change (cos 1.0 at
6,706 tokens). Every weight the fused graph shares with BAAI's `onnx/model.onnx_data` points back
at that file, unchanged; only the 24 layers' Q, K and V biases, concatenated, are inline. The
graph declares `attention_mask` again, unused, because fastembed feeds it.

Run once, by hand, from BAAI/bge-m3 at commit 5617a9f61b028005a4858fdac845db406aefb181, with
Python 3.12, onnxruntime 1.28.0 (the version ort 2.0.0-rc.13 links), onnx 1.23.2, sympy 1.14.0,
numpy 2.5.3 and protobuf 7.36.2. MODEL_DIR holds `onnx/model.onnx`,
`onnx/Constant_7_attr__value` and `onnx/model.onnx_data`, each with one link (onnx refuses
external data that has several):

    python fuse.py MODEL_DIR assets/bge-m3-mha.onnx

The output's SHA-256 is pinned in src/embed_local.rs (`GRAPH_SHA256`); two runs gave the same.
"""

import argparse
import os

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper
from onnxruntime.transformers.fusion_options import FusionOptions
from onnxruntime.transformers.optimizer import optimize_model

parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
parser.add_argument("model_dir", help="BAAI/bge-m3's files at the pinned commit")
parser.add_argument("out", help="the fused graph to write")
args = parser.parse_args()
model_dir, out = args.model_dir, args.out
options = FusionOptions("bert")
options.use_multi_head_attention = True
options.disable_attention_mask()
fused = optimize_model(
    os.path.join(model_dir, "onnx", "model.onnx"),
    model_type="bert",
    num_heads=16,
    hidden_size=1024,
    opt_level=0,
    optimization_options=options,
).model

# BAAI's external weights: their offsets in model.onnx_data, found again by their bytes.
original = onnx.load(os.path.join(model_dir, "onnx", "model.onnx"), load_external_data=False)
data = np.memmap(os.path.join(model_dir, "onnx", "model.onnx_data"), dtype=np.uint8, mode="r")
by_head = {}
for t in original.graph.initializer:
    if t.data_location == TensorProto.EXTERNAL:
        ext = {e.key: e.value for e in t.external_data}
        if ext["location"] == "model.onnx_data":
            offset, length = int(ext.get("offset", 0)), int(ext["length"])
            by_head.setdefault((length, bytes(data[offset : offset + min(length, 4096)])), []).append((offset, length))

rebound = inline = inline_bytes = 0
for t in fused.graph.initializer:
    raw = numpy_helper.to_array(t).tobytes()
    at = next(
        (o for o in by_head.get((len(raw), raw[:4096]), []) if bytes(data[o[0] : o[0] + o[1]]) == raw),
        None,
    )
    for field in ("raw_data", "float_data", "int64_data", "int32_data", "double_data"):
        t.ClearField(field)
    del t.external_data[:]
    if at:
        t.data_location = TensorProto.EXTERNAL
        for key, value in (("location", "model.onnx_data"), ("offset", str(at[0])), ("length", str(at[1]))):
            entry = t.external_data.add()
            entry.key, entry.value = key, value
        rebound += 1
    else:
        t.data_location = TensorProto.DEFAULT
        t.raw_data = raw
        inline += 1
        inline_bytes += len(raw)

if not any(i.name == "attention_mask" for i in fused.graph.input):
    fused.graph.input.append(
        helper.make_tensor_value_info("attention_mask", TensorProto.INT64, ["batch_size", "sequence_length"])
    )
onnx.save_model(fused, out, save_as_external_data=False)
ops = {}
for n in fused.graph.node:
    ops[n.op_type] = ops.get(n.op_type, 0) + 1
print(f"weights in model.onnx_data {rebound}, inline {inline} ({inline_bytes} bytes); graph {os.path.getsize(out)} bytes")
print(f"MultiHeadAttention {ops.get('MultiHeadAttention', 0)}, Softmax {ops.get('Softmax', 0)}")
