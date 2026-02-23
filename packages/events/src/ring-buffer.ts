/**
 * Fixed-capacity ring buffer for efficient event replay storage.
 * When full, new items overwrite the oldest entries.
 */
export class RingBuffer<T> {
  private readonly _buffer: (T | undefined)[];
  private readonly _capacity: number;
  private _head = 0;
  private _size = 0;

  constructor(capacity: number) {
    if (capacity < 1) {
      throw new Error('RingBuffer capacity must be at least 1');
    }
    this._capacity = capacity;
    this._buffer = new Array<T | undefined>(capacity);
  }

  /** Add an item to the buffer, overwriting the oldest if full. */
  push(item: T): void {
    this._buffer[this._head] = item;
    this._head = (this._head + 1) % this._capacity;
    if (this._size < this._capacity) {
      this._size++;
    }
  }

  /** Return all items in insertion order (oldest first). */
  toArray(): T[] {
    if (this._size === 0) {
      return [];
    }
    const result: T[] = [];
    const start =
      this._size < this._capacity
        ? 0
        : this._head;
    for (let i = 0; i < this._size; i++) {
      const index = (start + i) % this._capacity;
      result.push(this._buffer[index] as T);
    }
    return result;
  }

  /** Number of items currently stored. */
  get size(): number {
    return this._size;
  }

  /** Maximum number of items the buffer can hold. */
  get capacity(): number {
    return this._capacity;
  }

  /** Remove all items from the buffer. */
  clear(): void {
    this._buffer.fill(undefined);
    this._head = 0;
    this._size = 0;
  }
}
