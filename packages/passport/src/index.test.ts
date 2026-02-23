import { describe, test, expect } from 'bun:test';
import {
  generateKeypair,
  keypairFromPrivateKey,
  isValidPrivateKey,
  isValidPublicKey,
  serializeKeypair,
  deserializeKeypair,
  publicKeyToDID,
  didToPublicKey,
  isValidDID,
  parseDID,
  sign,
  verify,
  signMessage,
  verifyMessage,
  createPassport,
  verifyPassport,
  serializePassport,
  deserializePassport,
  generateDID,
  signPassport,
  toHex,
  fromHex,
} from './index';

describe('Utils', () => {
  test('toHex and fromHex roundtrip', () => {
    const bytes = new Uint8Array([0, 1, 255, 128, 64]);
    const hex = toHex(bytes);
    expect(hex).toBe('0001ff8040');
    const back = fromHex(hex);
    expect(back).toEqual(bytes);
  });

  test('fromHex rejects invalid hex', () => {
    expect(() => fromHex('xyz')).toThrow();
    expect(() => fromHex('0')).toThrow(); // odd length
  });
});

describe('Keypair', () => {
  test('generateKeypair returns valid 32-byte keys', () => {
    const kp = generateKeypair();
    expect(kp.publicKey.length).toBe(32);
    expect(kp.privateKey.length).toBe(32);
  });

  test('keypairFromPrivateKey derives matching public key', () => {
    const kp1 = generateKeypair();
    const kp2 = keypairFromPrivateKey(kp1.privateKey);
    expect(toHex(kp2.publicKey)).toBe(toHex(kp1.publicKey));
  });

  test('keypairFromPrivateKey rejects invalid key', () => {
    expect(() => keypairFromPrivateKey(new Uint8Array(16))).toThrow();
    expect(() => keypairFromPrivateKey(new Uint8Array(32))).toThrow(); // all zeros
  });

  test('isValidPrivateKey', () => {
    const kp = generateKeypair();
    expect(isValidPrivateKey(kp.privateKey)).toBe(true);
    expect(isValidPrivateKey(null)).toBe(false);
    expect(isValidPrivateKey(new Uint8Array(32))).toBe(false); // all zeros
    expect(isValidPrivateKey(new Uint8Array(16))).toBe(false); // wrong length
  });

  test('isValidPublicKey', () => {
    const kp = generateKeypair();
    expect(isValidPublicKey(kp.publicKey)).toBe(true);
    expect(isValidPublicKey(null)).toBe(false);
    expect(isValidPublicKey(new Uint8Array(32))).toBe(false); // all zeros
  });

  test('serialize and deserialize keypair', () => {
    const kp = generateKeypair();
    const serialized = serializeKeypair(kp);
    expect(typeof serialized.publicKey).toBe('string');
    expect(typeof serialized.privateKey).toBe('string');

    const deserialized = deserializeKeypair(serialized);
    expect(toHex(deserialized.publicKey)).toBe(toHex(kp.publicKey));
    expect(toHex(deserialized.privateKey)).toBe(toHex(kp.privateKey));
  });
});

describe('DID', () => {
  test('publicKeyToDID produces valid DID format', () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    expect(did).toMatch(/^did:key:z[1-9A-HJ-NP-Za-km-z]+$/);
  });

  test('didToPublicKey roundtrip', () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    const extracted = didToPublicKey(did);
    expect(toHex(extracted)).toBe(toHex(kp.publicKey));
  });

  test('isValidDID', () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    expect(isValidDID(did)).toBe(true);
    expect(isValidDID('not-a-did')).toBe(false);
    expect(isValidDID('did:key:abc')).toBe(false); // too short
    expect(isValidDID(123)).toBe(false);
  });

  test('parseDID returns method and public key', () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    const parsed = parseDID(did);
    expect(parsed.method).toBe('key');
    expect(toHex(parsed.publicKey)).toBe(toHex(kp.publicKey));
  });

  test('publicKeyToDID rejects wrong-length key', () => {
    expect(() => publicKeyToDID(new Uint8Array(16))).toThrow('Public key must be 32 bytes');
  });
});

describe('Signature', () => {
  test('sign and verify raw bytes', async () => {
    const kp = generateKeypair();
    const message = new TextEncoder().encode('hello world');
    const sig = await sign(message, kp.privateKey);
    expect(sig.length).toBe(64);

    const valid = await verify(sig, message, kp.publicKey);
    expect(valid).toBe(true);
  });

  test('verify rejects tampered message', async () => {
    const kp = generateKeypair();
    const message = new TextEncoder().encode('hello world');
    const sig = await sign(message, kp.privateKey);

    const tampered = new TextEncoder().encode('hello world!');
    const valid = await verify(sig, tampered, kp.publicKey);
    expect(valid).toBe(false);
  });

  test('verify rejects wrong public key', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();
    const message = new TextEncoder().encode('hello');
    const sig = await sign(message, kp1.privateKey);

    const valid = await verify(sig, message, kp2.publicKey);
    expect(valid).toBe(false);
  });

  test('signMessage and verifyMessage with JSON payload', async () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    const payload = { action: 'navigate', target: { x: 10, y: 20 } };

    const sig = await signMessage(payload, kp.privateKey);
    expect(typeof sig).toBe('string');
    expect(sig.length).toBe(128);

    const valid = await verifyMessage(did, sig, payload);
    expect(valid).toBe(true);
  });

  test('verifyMessage rejects tampered payload', async () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);
    const payload = { action: 'navigate' };

    const sig = await signMessage(payload, kp.privateKey);
    const valid = await verifyMessage(did, sig, { action: 'stop' });
    expect(valid).toBe(false);
  });

  test('verifyMessage rejects invalid signature format', async () => {
    const kp = generateKeypair();
    const did = publicKeyToDID(kp.publicKey);

    expect(await verifyMessage(did, 'not-hex', {})).toBe(false);
    expect(await verifyMessage(did, '', {})).toBe(false);
  });
});

describe('Passport', () => {
  test('createPassport and verifyPassport roundtrip', async () => {
    const kp = generateKeypair();
    const passport = await createPassport(kp, ['navigate', 'perceive']);

    expect(passport.did).toMatch(/^did:key:z/);
    expect(passport.capabilities).toEqual(['navigate', 'perceive']);
    expect(typeof passport.issuedAt).toBe('number');
    expect(passport.publicKey.length).toBe(64); // 32 bytes hex
    expect(passport.signature.length).toBe(128); // 64 bytes hex

    const valid = await verifyPassport(passport);
    expect(valid).toBe(true);
  });

  test('verifyPassport rejects tampered passport', async () => {
    const kp = generateKeypair();
    const passport = await createPassport(kp, ['navigate']);

    const tampered = { ...passport, capabilities: ['navigate', 'hack'] };
    const valid = await verifyPassport(tampered);
    expect(valid).toBe(false);
  });

  test('verifyPassport rejects mismatched DID', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();
    const passport = await createPassport(kp1, ['navigate']);

    // Replace DID with a different one
    const fakeDid = publicKeyToDID(kp2.publicKey);
    const tampered = { ...passport, did: fakeDid };
    const valid = await verifyPassport(tampered);
    expect(valid).toBe(false);
  });

  test('serialize and deserialize passport', async () => {
    const kp = generateKeypair();
    const passport = await createPassport(kp, ['reason']);

    const json = serializePassport(passport);
    expect(typeof json).toBe('string');

    const restored = deserializePassport(json);
    expect(restored.did).toBe(passport.did);
    expect(restored.capabilities).toEqual(passport.capabilities);
    expect(restored.signature).toBe(passport.signature);

    const valid = await verifyPassport(restored);
    expect(valid).toBe(true);
  });

  test('deserializePassport rejects invalid JSON', () => {
    expect(() => deserializePassport('not json')).toThrow();
    expect(() => deserializePassport('{}')).toThrow();
  });
});

describe('Convenience API', () => {
  test('generateDID returns valid DID and private key', () => {
    const { did, privateKey } = generateDID();
    expect(did).toMatch(/^did:key:z/);
    expect(privateKey.length).toBe(32);
    expect(isValidDID(did)).toBe(true);
  });

  test('signPassport creates verifiable passport', async () => {
    const { privateKey } = generateDID();
    const passport = await signPassport(
      { capabilities: ['navigate', 'communicate'] },
      privateKey
    );

    expect(passport.capabilities).toEqual(['navigate', 'communicate']);
    const valid = await verifyPassport(passport);
    expect(valid).toBe(true);
  });
});
