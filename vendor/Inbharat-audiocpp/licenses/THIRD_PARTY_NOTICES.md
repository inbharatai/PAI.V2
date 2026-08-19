# Third-party notices — local RC1

The default InBharat Audio RC1 build contains **no copied audio.cpp source, ggml source, model weights, tokenizer files, datasets, demo voices, or bundled third-party assets**. It uses the platform C/C++ runtime, threading, filesystem, and dynamic-loader facilities supplied by the build/runtime environment.

The separately located audio.cpp checkout at commit `bb15edd78b56e035967e0eb999a6b28a62337db4` was reviewed as architecture/provenance input but is not linked or redistributed by the default build. Its optional scaffold remains `DEFERRED`; enabling the scaffold still copies no source. If selected audio.cpp or ggml files are later shipped, regenerate this notice from the exact selected closure and retain Apache-2.0/MIT/BSD notices as applicable.

Android builds link NDK system libraries (`liblog`, `libandroid`, `libdl`, pthread/libc facilities) and one consistent C++ runtime. Platform SDK/runtime licensing is governed by the corresponding platform distribution.

This file is an engineering inventory, not legal advice. External distribution remains subject to counsel and packaging review.
