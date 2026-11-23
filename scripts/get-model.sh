#!/bin/sh
# Downloads a whisper.cpp model to ~/.textcut/models (textcut uses the largest one there).
#   scripts/get-model.sh            -> large-v3-turbo (1.6 GB, best quality, all languages)
#   scripts/get-model.sh small      -> small (466 MB, faster, all languages)
#   scripts/get-model.sh base.en    -> base.en (142 MB, English only)
set -e
name="${1:-large-v3-turbo}"
dir="$HOME/.textcut/models"
mkdir -p "$dir"
url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$name.bin"
echo "Downloading ggml-$name.bin to $dir"
curl -L --fail --progress-bar -C - -o "$dir/ggml-$name.bin" "$url"
echo "Done."
