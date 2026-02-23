# IMA xattr Preservation Test Report

**Date**: 2026-02-23
**Task**: `ima-sign`
**Purpose**: Validate IMA signature preservation through tar+HTTP roundtrip

## Executive Summary

✅ **All tests passed**: IMA signatures (`security.ima` xattr) are preserved when using `tar --xattrs --xattrs-include='*'`, and correctly fail verification when xattrs are omitted.

## Test Scenarios

### 1. Positive Test: xattr Preservation via tar+HTTP

**Process**:
```
Create files → IMA sign → tar --xattrs --xattrs-include='*' → HTTP upload
→ HTTP download → tar extract --xattrs → IMA verify
```

**Result**: ✅ **PASS**

**Evidence** (`ima-http-roundtrip.log`):
```
[*] Verifying extracted sample-a.txt
hash(sha256): e65b7034bd5c90d6887201e6450421b4d77103317b79c67ceb350225693728fc
/tmp/vmize-worker/work/ima-sign/extract/sample-a.txt: verification is OK

[*] Verifying extracted sample-b.bin
hash(sha256): 348a8b868a61ffac969aec854bcc3500036e56d4be359a5d60db4e5250443ca6
/tmp/vmize-worker/work/ima-sign/extract/sample-b.bin: verification is OK
```

**xattr Comparison** (`signed-xattr.txt`):

| File | Original xattr | Roundtrip xattr | Match |
|------|----------------|-----------------|-------|
| sample-a.txt | `security.ima=0x0302043bbf5d90010058f69d...` | `security.ima=0x0302043bbf5d90010058f69d...` | ✅ Yes |
| sample-b.bin | `security.ima=0x0302043bbf5d900100419d8e...` | `security.ima=0x0302043bbf5d900100419d8e...` | ✅ Yes |

**Key Finding**: HTTP transmission does not affect xattr preservation. The tar format with `--xattrs` flags preserves IMA signatures.

---

### 2. Tampered File Detection

**Process**:
```
Sign file → Modify content → Verify signature
```

**Result**: ✅ **PASS** (detection successful)

**Evidence** (`ima-negative.log`):
```
[*] Verifying tampered file /tmp/vmize-worker/work/ima-sign/sample-a-tampered.txt
[*] Tampered verification failed as expected (status=1)
```

**Key Finding**: IMA verification correctly detects file tampering (content modification after signing).

---

### 3. Negative Test: xattr Omission

**Process**:
```
Create files → IMA sign → tar (WITHOUT --xattrs) → HTTP upload
→ HTTP download → tar extract → IMA verify
```

**Result**: ✅ **PASS** (verification failed as expected)

**Evidence** (`ima-no-xattr.log`):
```
[*] Negative test: packing without xattrs
[*] Extracting no-xattr tar (without xattrs)
[*] Attempting to verify extracted sample-a.txt
getxattr failed: /tmp/vmize-worker/work/ima-sign/no-xattr-extract/sample-a.txt
errno: No data available (61)
[*] No-xattr verification failed as expected (status=1)

[*] Attempting to verify extracted sample-b.bin
getxattr failed: /tmp/vmize-worker/work/ima-sign/no-xattr-extract/sample-b.bin
errno: No data available (61)
[*] No-xattr verification failed as expected (status=1)
```

**xattr Check** (`signed-xattr.txt`):
```
### no-xattr:sample-a.txt
/tmp/vmize-worker/work/ima-sign/no-xattr-extract/sample-a.txt: security.ima: No such attribute

### no-xattr:sample-b.bin
/tmp/vmize-worker/work/ima-sign/no-xattr-extract/sample-b.bin: security.ima: No such attribute
```

**Key Finding**: Without `--xattrs --xattrs-include='*'`, IMA signatures are **completely lost** during tar archiving.

---

## Critical Requirements

### ✅ Required for xattr Preservation

1. **Packing**:
   ```bash
   tar --xattrs --xattrs-include='*' --format=posix -cpf archive.tar files
   ```

2. **Extracting**:
   ```bash
   tar --xattrs --xattrs-include='*' -xpf archive.tar
   ```

### ❌ Causes xattr Loss

- Using `tar` without `--xattrs` flag
- Using `tar` without `--xattrs-include='*'` (may skip some xattr namespaces)
- Using compression tools that don't support xattrs (e.g., basic `zip`)

---

## Artifacts Generated

| File | Size | Description |
|------|------|-------------|
| `cert.der` | 791B | Verification certificate |
| `sample-a.txt` | 19B | Test file (text) |
| `sample-b.bin` | 512B | Test file (binary) |
| `signed-http.tar` | 10K | Archive with xattrs |
| `downloaded-signed-http.tar` | 10K | Downloaded archive (with xattrs) |
| `no-xattr.tar` | 10K | Archive without xattrs |
| `downloaded-no-xattr.tar` | 10K | Downloaded archive (without xattrs) |
| `ima-sign.log` | 1.4K | Signing log |
| `ima-verify.log` | 648B | Verification log |
| `ima-negative.log` | 346B | Tamper detection log |
| `ima-http-roundtrip.log` | 781B | HTTP roundtrip log |
| `ima-no-xattr.log` | 931B | xattr omission log |
| `signed-xattr.txt` | 3.0K | xattr dump (all scenarios) |
| `ima-sign-summary.txt` | 359B | Test summary |

---

## Test Environment

- **Platform**: Linux VM (Ubuntu 24.04)
- **Tools**: `evmctl` (ima-evm-utils 1.4), `tar`, `curl`, `python3` HTTP server
- **IMA Mode**: Debug-verify only (no kernel appraise enforcement)
- **Disk Size**: 20G

---

## Summary Table

| Test Case | Expected Result | Actual Result | Status |
|-----------|----------------|---------------|--------|
| xattr preservation via tar+HTTP | Verify success | Verify success | ✅ PASS |
| Tampered file detection | Verify failure | Verify failure (status=1) | ✅ PASS |
| xattr omission | Verify failure | Verify failure (errno=61) | ✅ PASS |

---

## Recommendations

1. **Always use `--xattrs --xattrs-include='*'`** when archiving IMA-signed files
2. **Extract with `--xattrs`** to restore extended attributes
3. **Validate xattrs after transfer** using `getfattr -n security.ima <file>`
4. **Use POSIX tar format** (`--format=posix`) for better xattr compatibility

---

## References

- Task definition: `worker/example/ima-sign/task.json`
- Test script: `worker/example/ima-sign/input/10_sign_verify.sh`
- Output directory: `worker/example/ima-sign/output/`
- Tutorial: `worker/TASK_CHAIN_TUTORIAL.md`
