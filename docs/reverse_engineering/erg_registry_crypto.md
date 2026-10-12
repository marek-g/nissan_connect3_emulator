# ERG (encrypted registry) format and key chain

Findings from Oct-12 static attack on `ni2k_*/NAVAPP.ERG` + `DAPIAPP.ERG`
(firmware `D605_registry/` and SD `CRYPTNAV/DNL/BIN/NAV/` — byte-identical,
md5 `71442479c0af17540cb1717c56eea0f2` for eur NAVAPP.ERG in both places).

## File format (confirmed structurally, without the key)

| field | evidence |
|---|---|
| 12-byte header | every pair of ERG files diverges first at byte 12 (`lcp == 12` for all cross-region/cross-app pairs); header = u32 1 (algo/type), u32 1 (version), u32 0 (flags?), then |
| u32 at +12 | plaintext length: eur NAVAPP `0x0b7c`, nar `0x0c87`, eur DAPIAPP `0x0424` |
| payload | `(filesize − 12) == 16 * (len_at_12 + 2)` exactly for all files ⇒ **AES-128, 16-byte blocks, payload aligned at offset 12** |
| deterministic cipher | `ni2k_chn/NAVAPP.ERG` and `ni2k_aus/NAVAPP.ERG` are byte-identical (same plaintext, no per-file IV); eur/nar/chn NAVAPP + eur DAPIAPP share an identical ciphertext prefix bytes 12..223 (13 full blocks of shared plaintext) ⇒ fixed IV/ECB, **no random IV** |

## Key chain (from `procmwlx_out.out`, the only Linux binary with a crypted-file copier)

`fc_ups::FileUtils::copyFile()` (0x56b528, "copy while decrypting", used by the
DNL update path) is the only caller of the whole chain:

1. `KdsUtils::open()` (0x57f7bc) — `OSAL_IOOpen("/dev/kds", O_RDWR)`, retry ≤5.
   libosal implements `/dev/kds` as an IOSC bridge (`KDS_SHME`/`KDS_MSGQ`) to the
   **Tengine (triton_dualos) KDS server**, which stores entries in NOR flash
   (`/dev/flashblock_kds`, strings `KDS-Write OK!`, `dl_tclFlashOperations::dnl_ReadKDS`
   in `triton_mid.bin`).
2. `KdsUtils::pu8GetCryptedFileAesKey()` (0x57f8b8):
   * keyId = `0x1216` (if arg b1) else (`0x1217` if b2 else `0`);
   * `KdsUtils::ReadKDSEth(handle, keyId, 16, buf)` — buffer zeroed first;
   * `key[16] = kds16[16] XOR 03 05 06 07 08 0a 0b 0c 0e 87 61 87 62 87 6f 00`
     (static mask, VA 0x878c0c);
   * on KDS-open failure the zeroed buffer is used ⇒ key = mask (emulator-friendly
     fallback, but *not* the key of the shipped files — mask-as-key tested in
     ECB/CBC/CFB/OFB at offsets 12/16/224/236: no plaintext).
3. `BPCL_AES_Decrypt(1 /*mode=ECB*/, key, nblocks−1, 0, out)` — copyFile passes
   literal `1`, i.e. **AES-128 ECB** per 500-byte chunk (aes_decrypt in libosal
   0x485449c0 dispatches type 1=ECB, 2=CBC, 3=CFB, 4=OFB/CTR, 5=DES).

## Where the plaintext registry tree lives

`container.iso.bin` (DNL update stick, at `Firmware/D605/`) contains a full
`nor0/registry/` copy (mkisofs ISO, list with `7z l`): all plain `.reg` files
plus `StartConfig1/2/3.reg`, `hminavserver.reg`, `sdsapp.reg`, `map.reg`
(ni2k_58/70) and the *same* encrypted `.ERG` files. The `.ERG`s are installed
as-is; decryption happens at use-time via the KDS key above.

## Why static decryption failed

The 16 bytes of KDS entries `0x1216`/`0x1217` are written per-vehicle (or per
family) at factory into the KDS flash block. No KDS content dump exists in
anything we have (kernel `uimage.bin` has no kds driver strings — /dev/kds is
bridged from Tengine; tengine `telf_*` fragments in `triton_mid.bin` contain no
AES tables; `reg_eur.tar.gz.bin` carries only TTS voices; ISO carries only the
encrypted copies). With KDS bytes unknown, `key = kds16 XOR mask` is
unknowable ⇒ **the shipped ERGs cannot be decrypted statically from our data**.

## Implications for the emulator

* We must **not** expect to load real `NAVAPP.ERG` content. Region-specific
  values (FC_STAT/FC_TEA knobs etc.) have to be reconstructed from PROCNAV
  strings + failure logs into `data/registry_compat.reg` (path 1 of the plan).
* If we ever need real ERG content: read it honestly through the emulator —
  hook `BPCL_AES_Decrypt` (libosal is an allowed patch target) and log the key
  argument **plus** decrypted chunks while running the guest stack on a real
  KDS emulated with keys extracted from a live head unit (`/dev/kds` reads).
* ERG is confirmed *not* to carry the `PROCESS/CONFIG/PROCNAVI/SERVICES` scd
  data path we need (those keys must exist in the plain `StartConfig*.reg`
  family; `StartConfig1/2/3.reg` in the ISO are *new* files we never loaded —
  see `docs/issues/correctness.md`).

## Extraction plan from a live head unit (root + USB stick, any firmware)

Ranked by value; none of 1–2 requires the KDS key or matching firmware version:

1. **Decrypted registry tree.** The live `/dev/registry` content (IOSC shared
   memory, decrypted at boot) contains everything the ERGs would give. Dump
   `ipcs -m` + the registry shm segment, or better: a small static ARM tool
   that walks the tree through the same `/dev/registry` OSAL ioctls the
   emulator already serves and prints a `.reg` file. Version-independent.
2. **`/dev/ffs2/registry/`** — the installed `.reg`/`.ERG` files (may be newer
   than our ISO copies; also reveals files not present in D605).
3. **KDS key** (for completeness): entries `0x1216`/`0x1217` via `/dev/kds`
   reads (strace the reader, or a KDS read tool); `ls -l /proc/*/fd | grep
   kds`; KDS NOR partition if exposed as `/dev/mtd*` (`cat /proc/mtd`).
   AES key = entry bytes XOR `03050607080a0b0c0e87618762876f00`.
4. Do **not** copy app binaries expecting to switch the emulator to them:
   emulator hooks are keyed to `PROCNAV_dec.out` (D605) addresses; registry
   data is portable across builds, binaries are not.