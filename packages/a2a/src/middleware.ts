/**
 * Middleware system for A2A server
 */

import type { JsonRpcRequest, JsonRpcResponse } from './types';

/** Context passed through middleware chain */
export interface MiddlewareContext {
  /** The incoming JSON-RPC request */
  request: JsonRpcRequest;
  /** DID of the sender */
  from: string;
  /** Timestamp when request was received */
  receivedAt: number;
  /** Arbitrary metadata that middleware can attach */
  metadata: Map<string, unknown>;
}

/** Next function to call the next middleware or handler */
export type NextFunction = () => Promise<JsonRpcResponse>;

/** Middleware function signature */
export type Middleware = (
  ctx: MiddlewareContext,
  next: NextFunction,
) => Promise<JsonRpcResponse>;

/**
 * Compose multiple middleware functions into a single chain
 */
export function composeMiddleware(
  middlewares: Middleware[],
  finalHandler: (ctx: MiddlewareContext) => Promise<JsonRpcResponse>,
): (ctx: MiddlewareContext) => Promise<JsonRpcResponse> {
  return (ctx: MiddlewareContext) => {
    let index = -1;

    function dispatch(i: number): Promise<JsonRpcResponse> {
      if (i <= index) {
        return Promise.reject(new Error('next() called multiple times'));
      }
      index = i;

      if (i >= middlewares.length) {
        return finalHandler(ctx);
      }

      const mw = middlewares[i]!;
      return mw(ctx, () => dispatch(i + 1));
    }

    return dispatch(0);
  };
}

/**
 * Built-in rate limiting middleware
 */
export function rateLimiter(opts: {
  maxRequests: number;
  windowMs: number;
}): Middleware {
  const windows = new Map<string, { count: number; resetAt: number }>();

  return async (ctx, next) => {
    const key = ctx.from;
    const now = Date.now();
    let entry = windows.get(key);

    if (!entry || now >= entry.resetAt) {
      entry = { count: 0, resetAt: now + opts.windowMs };
      windows.set(key, entry);
    }

    entry.count++;

    if (entry.count > opts.maxRequests) {
      return {
        jsonrpc: '2.0' as const,
        error: {
          code: -32029,
          message: 'Rate limit exceeded',
        },
        id: ctx.request.id,
      };
    }

    return next();
  };
}

/**
 * Built-in logging middleware
 */
export function logger(logFn: (msg: string) => void = console.log): Middleware {
  return async (ctx, next) => {
    const start = Date.now();
    logFn(`[A2A] ${ctx.from} -> ${ctx.request.method}`);

    const response = await next();
    const duration = Date.now() - start;

    if (response.error) {
      logFn(`[A2A] ${ctx.request.method} error (${duration}ms): ${response.error.message}`);
    } else {
      logFn(`[A2A] ${ctx.request.method} ok (${duration}ms)`);
    }

    return response;
  };
}
