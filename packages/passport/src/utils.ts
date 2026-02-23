/**
 * Browser-compatible utility functions for encoding/decoding
 * Avoids Node.js Buffer to ensure browser compatibility
 */

/**
 * Convert Uint8Array to hex string
 * @param bytes - Byte array to encode
 * @returns Hex-encoded string (lowercase)
 */
export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map(byte => byte.toString(16).padStart(2, '0'))
    .join('');
}

/**
 * Convert hex string to Uint8Array
 * @param hex - Hex-encoded string
 * @returns Byte array
 * @throws Error if hex string is invalid
 */
export function fromHex(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) {
    throw new Error('Invalid hex string: length must be even');
  }

  if (!/^[0-9a-f]*$/i.test(hex)) {
    throw new Error('Invalid hex string: contains non-hex characters');
  }

  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16);
  }

  return bytes;
}
