/**
 * TFListener - Listens to /tf and /tf_static topics and maintains a
 * frame buffer of the latest transforms.
 */

import type { ROSTransformStamped } from '@qualia/types';
import { ROSBridgeClient } from './client.ts';

/** TF message as published on /tf (array of transforms) */
interface TFMessage {
  transforms: ROSTransformStamped[];
}

export class TFListener {
  private readonly _client: ROSBridgeClient;

  /**
   * Frame buffer: parentFrame -> childFrame -> latest transform.
   * Also stores inverse mapping childFrame -> parentFrame for lookups.
   */
  private readonly _frames = new Map<string, Map<string, ROSTransformStamped>>();

  /** Registered callbacks for all incoming transforms */
  private readonly _callbacks = new Set<(tf: ROSTransformStamped) => void>();

  constructor(client: ROSBridgeClient) {
    this._client = client;
  }

  /**
   * Start listening on /tf. Returns a stop function that unsubscribes.
   */
  listen(): () => void {
    const unsub = this._client.subscribe<TFMessage>(
      '/tf',
      'tf2_msgs/TFMessage',
      (msg) => {
        for (const tf of msg.transforms) {
          this._storeTransform(tf);
          for (const cb of this._callbacks) {
            cb(tf);
          }
        }
      },
    );

    return unsub;
  }

  /**
   * Look up the latest transform from sourceFrame to targetFrame.
   * Returns null if no transform is known between these frames.
   *
   * This performs a direct lookup only (parent->child). A full tree
   * walk is not implemented here to keep things simple.
   */
  getTransform(targetFrame: string, sourceFrame: string): ROSTransformStamped | null {
    // Try direct parent->child
    const children = this._frames.get(targetFrame);
    if (children) {
      const tf = children.get(sourceFrame);
      if (tf) return tf;
    }

    // Try reverse: sourceFrame is the parent, targetFrame is the child
    const reverseChildren = this._frames.get(sourceFrame);
    if (reverseChildren) {
      const tf = reverseChildren.get(targetFrame);
      if (tf) return tf;
    }

    return null;
  }

  /**
   * Register a callback that fires for every incoming transform.
   * Returns an unregister function.
   */
  onTransform(callback: (tf: ROSTransformStamped) => void): () => void {
    this._callbacks.add(callback);
    return () => {
      this._callbacks.delete(callback);
    };
  }

  // -----------------------------------------------------------------------
  // Internals
  // -----------------------------------------------------------------------

  private _storeTransform(tf: ROSTransformStamped): void {
    const parent = tf.header.frame_id;
    const child = tf.child_frame_id;

    let children = this._frames.get(parent);
    if (!children) {
      children = new Map();
      this._frames.set(parent, children);
    }
    children.set(child, tf);
  }
}
