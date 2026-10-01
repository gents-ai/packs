ocr pack: third-party notices

This pack embeds two text recognition models in its plugin. They are not
modified: they are the published rten-format files, byte for byte.

text-detection.rten
  Source:  https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten
  SHA-256: f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca
  Author:  Robert Knight (ocrs-models, https://github.com/robertknight/ocrs-models)
  Licence: Creative Commons Attribution-ShareAlike 4.0 International (CC BY-SA 4.0),
           https://creativecommons.org/licenses/by-sa/4.0/
           as stated on https://huggingface.co/robertknight/ocrs
  Trained on HierText (CC BY-SA 4.0) and synthetic data.

text-recognition.rten
  Source:  https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten
  SHA-256: e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e
  Author:  Robert Knight (ocrs-models, https://github.com/robertknight/ocrs-models)
  Licence: CC BY-SA 4.0, as above.
  Trained on HierText (CC BY-SA 4.0) and synthetic data.

Anyone redistributing the models, or a modified version of them, must keep this
notice, credit the author, and release the model files under CC BY-SA 4.0. The
plugin code around them is separate and is licensed as the rest of this repository.

The plugin links these Rust crates, each under its own permissive licence
(MIT, Apache-2.0 or both): ocrs, rten, hayro, hayro-interpret, hayro-syntax,
vello_cpu, kurbo, image, zip, roxmltree, quick-xml, html-escape, serde,
serde_json and base64. Run `cargo tree` in plugins/ocr for the full list.
