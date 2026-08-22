# Bundled target-speaker extraction model

The two ONNX files are a lossless graph split of the INT8 WeSep BSRNN ECAPA
VoxCeleb1 conversion. `wesep-speaker-encoder-int8.onnx` converts the three
guided enrollment samples into one protected 192-value target-speaker
embedding. `wesep-bsrnn-voicefilter-3s-int8.onnx` consumes that persistent
embedding and a three-second mixture STFT, returning the owner-only STFT used
by the hidden authoritative ASR stream after an enrolled wake is accepted.

- Encoder: `[1, 300, 80]` Kaldi fbank -> `[1, 192]` raw speaker logits
- Separator: `[1, 2, 257, 376]` float STFT plus `[1, 192]` speaker logits ->
  `[1, 2, 257, 376]` float target STFT
- Encoder SHA-256: `9CEC30564E3A87746BDD44108779EDDF5323CD9ED4BD0966B64883A6EEBBF46E`
- Separator SHA-256: `1BFA3C60EA58288DE6947C62D6A49FBA9AEEE20BFBCE0B0415504F506F3F20B4`
- Numerical split parity: encoder and separator outputs are element-for-element
  equal to the original unsplit ONNX graph for the same inputs.
- Upstream: <https://github.com/wenet-e2e/wesep>
- License and attribution: `wesep-bsrnn-voicefilter-3s-int8.LICENSE.txt`
