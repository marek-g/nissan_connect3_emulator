You are working on emulator of the Bosch lcn2kai car headunit. The goal is to have an usable emulator to run and test navigation system with GUI.

The emulator has access to the firmware: `/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz` and SD card content: `/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/NISSAN Connect LCN3 V7 2022_2023`. Do not ever modify the content located there.

Here you have Linux 2.6.34 source code: `/home/marek/Ext/reverse_engineering/NissanMaps/linux-2.6.32.14`. Every time you are fixing or creating a new syscall, first try to understand how the syscall works exactly in Linux 2.6.32.14. Remember that we are emulating ARM 32-bit architecture, which has its own `arch/arm` folder with files that override many generic values (for example `fcntl.h`).

You can keep track of any issues (adding and removing them) with the following files:
- `docs/issues/correctness.md`
- `docs/issues/robustness.md`
- `docs/issues/performance.md`
- `docs/issues/design.md`

Commit to git frequently. Every time you make a meaningful change that you believe moves the emulator forward (a fix, a new hook, a new syscall, a working feature slice), commit it. Small, focused commits make it easy to bisect regressions later. Do not commit work-in-progress that does not build.
