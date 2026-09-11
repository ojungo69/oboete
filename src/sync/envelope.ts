// Bundle envelope `oboete-sync-bundle/1` (contracts/sync.md). Security-owned: Node `crypto`
// only, AES-256-GCM over fixed 64 KiB chunks with the age STREAM nonce rule (11-byte counter plus
// a final-chunk byte), the 44-byte prefix as AAD on every chunk, and per-bundle chunk keys from
// HKDF over the space key and a fresh salt. The reader is length-driven and needs no length
// field: truncation, reordering, splicing and a missing final chunk all fail authentication.
import { createCipheriv, createDecipheriv, createHash, hkdfSync, randomBytes } from 'node:crypto';
import { closeSync, fstatSync, fsyncSync, openSync, readSync, writeSync } from 'node:fs';

export const BUNDLE_MAGIC = Buffer.from('oboete-sync-bundle/1', 'ascii');
export const KEY_ID_BYTES = 8;
export const SALT_BYTES = 16;
export const PREFIX_BYTES = BUNDLE_MAGIC.length + KEY_ID_BYTES + SALT_BYTES;
export const CHUNK_BYTES = 65_536;
export const TAG_BYTES = 16;
export const MAX_PLAINTEXT_BYTES = 256 * 1024 * 1024;
export const MAX_CIPHERTEXT_BYTES = PREFIX_BYTES + MAX_PLAINTEXT_BYTES + TAG_BYTES * Math.ceil(MAX_PLAINTEXT_BYTES / CHUNK_BYTES);
const UNIT_BYTES = CHUNK_BYTES + TAG_BYTES;
const KEY_ID_INFO = 'oboete-sync-key-id/1';
const CHUNK_KEY_INFO = 'oboete-sync-bundle/1';

export type BundleErrorCode = 'oversize' | 'truncated' | 'bad_magic' | 'key_mismatch' | 'authentication_failed' | 'plaintext_too_large';

export class BundleError extends Error {
  constructor(readonly code: BundleErrorCode) {
    super(code);
    this.name = 'BundleError';
  }
}

export function keyId(spaceKey: Uint8Array): string {
  return Buffer.from(hkdfSync('sha256', spaceKey, Buffer.alloc(0), KEY_ID_INFO, KEY_ID_BYTES)).toString('hex');
}

function chunkKey(spaceKey: Uint8Array, salt: Buffer): Buffer {
  return Buffer.from(hkdfSync('sha256', spaceKey, salt, CHUNK_KEY_INFO, 32));
}

function nonce(counter: number, final: boolean): Buffer {
  const iv = Buffer.alloc(12);
  // 11-byte big-endian counter; JavaScript numbers cover far more chunks than any bundle holds.
  iv.writeUIntBE(counter, 5, 6);
  iv[11] = final ? 0x01 : 0x00;
  return iv;
}

function readFully(fd: number, length: number): Buffer {
  const buffer = Buffer.alloc(length);
  let offset = 0;
  while (offset < length) {
    const read = readSync(fd, buffer, offset, length - offset, null);
    if (read === 0) break;
    offset += read;
  }
  return buffer.subarray(0, offset);
}

/** Encrypts one plaintext file into one ciphertext file (not yet fsynced into place by rename). */
export function encryptBundle(spaceKey: Uint8Array, plaintextPath: string, ciphertextPath: string): { sha256: string; size: number } {
  const input = openSync(plaintextPath, 'r');
  try {
    const total = fstatSync(input).size;
    if (total > MAX_PLAINTEXT_BYTES) throw new BundleError('plaintext_too_large');
    const salt = randomBytes(SALT_BYTES);
    const prefix = Buffer.concat([BUNDLE_MAGIC, Buffer.from(keyId(spaceKey), 'hex'), salt]);
    const key = chunkKey(spaceKey, salt);
    const output = openSync(ciphertextPath, 'w', 0o600);
    const digest = createHash('sha256');
    let size = 0;
    const emit = (bytes: Buffer): void => { writeSync(output, bytes); digest.update(bytes); size += bytes.length; };
    try {
      emit(prefix);
      let remaining = total;
      let counter = 0;
      do {
        const length = Math.min(CHUNK_BYTES, remaining);
        const chunk = readFully(input, length);
        if (chunk.length !== length) throw new BundleError('truncated');
        remaining -= length;
        const cipher = createCipheriv('aes-256-gcm', key, nonce(counter, remaining === 0), { authTagLength: TAG_BYTES });
        cipher.setAAD(prefix);
        emit(Buffer.concat([cipher.update(chunk), cipher.final(), cipher.getAuthTag()]));
        counter += 1;
      } while (remaining > 0);
      fsyncSync(output);
    } finally { closeSync(output); }
    return { sha256: digest.digest('hex'), size };
  } finally { closeSync(input); }
}

/**
 * Decrypts one ciphertext file into a plaintext file. The output is written chunk by chunk but
 * the caller must treat it as garbage unless this returns: a failure at any chunk throws, and
 * nothing partial is ever applied (the caller deletes the file on throw).
 */
export function decryptBundle(
  spaceKey: Uint8Array, ciphertextPath: string, plaintextPath: string,
  options: { maxCiphertextBytes?: number } = {},
): { plaintextBytes: number } {
  const input = openSync(ciphertextPath, 'r');
  try {
    const size = fstatSync(input).size;
    if (size > (options.maxCiphertextBytes ?? MAX_CIPHERTEXT_BYTES)) throw new BundleError('oversize');
    if (size < PREFIX_BYTES + TAG_BYTES) throw new BundleError('truncated');
    const prefix = readFully(input, PREFIX_BYTES);
    if (!prefix.subarray(0, BUNDLE_MAGIC.length).equals(BUNDLE_MAGIC)) throw new BundleError('bad_magic');
    if (prefix.subarray(BUNDLE_MAGIC.length, BUNDLE_MAGIC.length + KEY_ID_BYTES).toString('hex') !== keyId(spaceKey)) {
      throw new BundleError('key_mismatch');
    }
    const key = chunkKey(spaceKey, prefix.subarray(BUNDLE_MAGIC.length + KEY_ID_BYTES));
    const output = openSync(plaintextPath, 'w', 0o600);
    let plaintextBytes = 0;
    try {
      let remaining = size - PREFIX_BYTES;
      let counter = 0;
      while (remaining > 0) {
        const final = remaining <= UNIT_BYTES;
        const unit = readFully(input, final ? remaining : UNIT_BYTES);
        if (unit.length !== (final ? remaining : UNIT_BYTES)) throw new BundleError('truncated');
        remaining -= unit.length;
        if (unit.length < TAG_BYTES) throw new BundleError('authentication_failed');
        const decipher = createDecipheriv('aes-256-gcm', key, nonce(counter, final), { authTagLength: TAG_BYTES });
        decipher.setAAD(prefix);
        decipher.setAuthTag(unit.subarray(unit.length - TAG_BYTES));
        let chunk: Buffer;
        try {
          chunk = Buffer.concat([decipher.update(unit.subarray(0, unit.length - TAG_BYTES)), decipher.final()]);
        } catch { throw new BundleError('authentication_failed'); }
        writeSync(output, chunk);
        plaintextBytes += chunk.length;
        counter += 1;
      }
    } finally { closeSync(output); }
    return { plaintextBytes };
  } finally { closeSync(input); }
}
