# Bundled target-speaker extraction model

`wesep-bsrnn-voicefilter-3s-int8.onnx` is an INT8 ONNX conversion of the WeSep
BSRNN ECAPA VoxCeleb1 checkpoint. It receives a three-second mixture STFT and
an owner-enrollment fbank, and returns the owner-only STFT used by the hidden
authoritative ASR stream after an enrolled wake has already been accepted.

- Mixture input: `[1, 2, 257, 376]` float STFT (16 kHz, FFT 512, hop 128)
- Enrollment input: `[1, 300, 80]` Kaldi fbank
- Output: `[1, 2, 257, 376]` float target STFT
- SHA-256: `10D709E513DD3C18351ABDD1342AC53C032738E32692A74552946DB5896DD6A9`
- Upstream: <https://github.com/wenet-e2e/wesep>
- License and attribution: `wesep-bsrnn-voicefilter-3s-int8.LICENSE.txt`
