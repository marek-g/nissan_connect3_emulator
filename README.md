# Nissan Connect 3 Emulator

This project is an attempt to emulate and run firmware from Nissan Qashqai 2017. My aim is to be able to run Navigation application to help me verify navigation data hacks (finally I would like to be able to use OSM maps with Nissan Navigation system).

_**It's in a very early stage and the project is not usable yet**_. And probably never will be because of the amout of work needed. But it is fun to start and learn about executable and operating system interaction.

Similar projects:
- https://github.com/zeropointdynamics/zelos (Python)
- https://github.com/qilingframework/qiling (Python)
- https://github.com/ant4g0nist/rudroid (Rust, Android, ARM64) - looks like it is a copy of Qiling code (but with bugs)
- https://github.com/lunixbochs/usercorn (Go)
- https://github.com/AeonLucid/AndroidNativeEmu (Python), elf loader: https://github.com/AeonLucid/AndroidNativeEmu/blob/40b89c8095b2aeb4a9f18ba9a853832afdb3d1b1/src/androidemu/internal/modules.py

Loading ELF:
- https://github.com/qilingframework/qiling/blob/master/qiling/loader/elf.py (Qiling)
- https://github.com/ant4g0nist/rudroid/blob/main/code/src/core/loaders/elfLoader.rs (rudroid) - be carrefour because of bugs
- https://github.com/torvalds/linux/blob/master/fs/binfmt_elf.c (Linux)

ELF format description:
- [ELF for the ARM® 64-bit
  Architecture (PDF)](http://45.32.102.46/files/learning/elf_for_arm.pdf)
- https://wiki.osdev.org/ELF_Tutorial
- https://www.caichinger.com/elf.html
- https://docs.oracle.com/cd/E26502_01/html/E26507/glcfv.html#scrolltoc
- https://refspecs.linuxfoundation.org/elf/elf.pdf

About ELF Auxiliary Vectors: http://articles.manugarg.com/aboutelfauxiliaryvectors.html

Special thanks to: https://github.com/raburton/lcn-patcher & https://github.com/sapphire-bt/lcn2kai-decompress.
