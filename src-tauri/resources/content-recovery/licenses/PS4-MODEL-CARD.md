---
license: apache-2.0
language:
- zh
- en
tags:
- audio
- speech
- target-speaker-extraction
- speaker-extraction
- bsrnn
- ecapa-tdnn
- whisper
datasets:
- TaurenMountain/REAL-PS4
pipeline_tag: audio-to-audio
---

# PS4: Proxy-Supervised Joint Training for Real Target Speaker Extraction

PS4 is a **Target Speaker Extraction (TSE)** model that jointly optimizes speech separation quality and ASR transcription accuracy through proxy supervision. It is fine-tuned from a pretrained BSRNN + ECAPA-TDNN backbone on the [REAL-PS4](https://huggingface.co/datasets/TaurenMountain/REAL-PS4) dataset — a multi-domain real-recording corpus covering Chinese and English meeting scenarios.

## Model Architecture

| Component | Details |
|-----------|---------|
| Separation backbone | BSRNN (Band-Split RNN), `feature_dim=128`, `num_repeat=6`, `stride=128`, `win=512` |
| Speaker encoder | ECAPA-TDNN (`ECAPA_TDNN_GLOB_c512`), `embed_dim=192`, `feat_dim=80`, ASTP pooling |
| Speaker fusion | Element-wise multiply (`spk_fuse_type: multiply`) |
| ASR backbone (proxy) | Whisper large-v3 (frozen, used only during training) |
| Speaker encoders (eval) | ResNet34-LM — `voxceleb_resnet34_LM` (EN), `cnceleb_resnet34_LM` (ZH) |
| Sample rate | 16 kHz |

## Training

### Loss Function

PS4 uses a **combined proxy-supervised loss** that simultaneously optimizes four objectives:

```
L = λ_ce · L_CE  +  λ_sim · L_sim  +  λ_vad · L_VAD  +  λ_dnsmos · L_DNSMOS
```

| Loss term | Weight | Description |
|-----------|--------|-------------|
| `L_CE` | 1.0 | ASR cross-entropy loss (Whisper large-v3 teacher-forcing) |
| `L_sim` | 5.0 | Speaker similarity ranking loss: `hinge(margin − (sim(tse,enroll) − sim(mix,enroll)))`, margin=0.5 |
| `L_VAD` | 0.5 | Target speaker activity detection loss (frame-level energy supervision) |
| `L_DNSMOS` | 0.2 | Differentiable DNSMOS-OVRL loss (ONNX model, no reference audio needed) |

### Training Data

Trained on [REAL-PS4](https://huggingface.co/datasets/TaurenMountain/REAL-PS4), which aggregates four real-recording meeting datasets:

| Dataset | Language | Scenario |
|---------|----------|----------|
| AISHELL-4 | Chinese | Multi-speaker meeting |
| AliMeeting | Chinese | Multi-speaker meeting |
| AMI | English | Multi-speaker meeting |
| CHiME6 | English | Dinner party / far-field |

### Hyperparameters

| Parameter | Value |
|-----------|-------|
| Optimizer | AdamW |
| Learning rate | 1e-5 |
| Weight decay | 1e-5 |
| LR scheduler | ExponentialDecrease (1e-5 → 1e-6) |
| Batch size | 1 |
| Max audio length | 30 s (480,000 samples) |
| Max enrollment length | 10 s (160,000 samples) |
| Gradient clipping | 5.0 |
| Precision | FP32 |
| Checkpoint | Epoch 37 |

## Usage

### Quick Start (recommended)

Use the included [`inference.py`](inference.py) — a self-contained script with absolutely no external dependencies beyond `torch`, `torchaudio`, and `numpy`:

```bash
# Install dependencies
pip install torch torchaudio numpy

# Single file extraction
python inference.py \
    --checkpoint checkpoint_epoch037.pt \
    --mix mix.wav \
    --enroll target_speaker.wav \
    --output result.wav

# Use GPU
python inference.py \
    --checkpoint checkpoint_epoch037.pt \
    --mix mix.wav \
    --enroll target.wav \
    --output result.wav \
    --device cuda

# Batch mode (process all .wav files in a directory)
python inference.py \
    --checkpoint checkpoint_epoch037.pt \
    --mix-dir ./mixtures/ \
    --enroll-dir ./enrollments/ \
    --output-dir ./results/ \
    --device cuda

# List available CUDA devices
python inference.py --list-devices
```

### Python API

```python
import torch
from inference import BSRNN, load_audio, extract_speaker, load_checkpoint, build_model

# Build model and load weights
device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
model = build_model(device)
load_checkpoint("checkpoint_epoch037.pt", model, device)

# Load audio (16 kHz mono)
mix = load_audio("mixture.wav")
enroll = load_audio("enrollment.wav")

# Run extraction
extracted = extract_speaker(model, mix, enroll, device)

# Save result
torchaudio.save("result.wav", extracted, 16000)
```

### Advanced: Manual Model Loading

```python
import torch
import torchaudio
from inference import BSRNN

# Build model with exact training config
model = BSRNN(
    feat_type="consistent",
    feature_dim=128,
    num_repeat=6,
    spk_emb_dim=192,
    spk_fuse_type="multiply",
    multi_fuse=False,
    spk_model="ECAPA_TDNN_GLOB_c512",
    sr=16000, win=512, stride=128,
    spk_args={"feat_dim": 80, "embed_dim": 192, "pooling_func": "ASTP"},
    spk_model_freeze=True,
    use_spk_transform=False,
    joint_training=True,
    multi_task=False,
    spk_feat=False,
)
model.eval()

# Load checkpoint
ckpt = torch.load("checkpoint_epoch037.pt", map_location="cpu")
state_dict = ckpt["model"] if "model" in ckpt else ckpt
model.load_state_dict(state_dict, strict=False)

# Run inference
mix, sr = torchaudio.load("mixture.wav")
enroll, sr = torchaudio.load("enrollment.wav")
with torch.no_grad():
    extracted, _ = model(mix, enroll)
```

## What's in this repository

| File | Description |
|------|-------------|
| `checkpoint_epoch037.pt` | PS4 model weights (epoch 37, proxy-supervised fine-tuned) |
| [`inference.py`](inference.py) | Self-contained inference script (no external ML libs required) |
| `README.md` | This file |

The training code, dataset, and full evaluation pipeline are available at:
- **Training code:** [GitHub - TaurenMountain/real-t](https://github.com/TaurenMountain/real-t) (see `opensource/code/`)
- **Dataset:** [TaurenMountain/REAL-PS4](https://huggingface.co/datasets/TaurenMountain/REAL-PS4)

## Citation

```bibtex
@misc{ning2026ps4,
  title         = {PS4: Proxy-Supervised Joint Training for Real Target Speaker Extraction},
  author        = {Wanyi Ning and Wei Zhou and Yingpeng Li and Yinshang Guo and Haitao Qian and Yiming Cheng},
  year          = {2026},
  eprint        = {2607.08111},
  archivePrefix = {arXiv},
  primaryClass  = {cs.SD},
  url           = {https://arxiv.org/abs/2607.08111}
}
```

## Related Resources

- **Dataset:** [TaurenMountain/REAL-PS4](https://huggingface.co/datasets/TaurenMountain/REAL-PS4)
