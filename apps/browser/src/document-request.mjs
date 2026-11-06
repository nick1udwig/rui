export async function documentRequest(api, path, data, signal, timeout = 15000) {
  const controller = new AbortController();
  let rejectAbort;
  const interrupted = new Promise((_, reject) => { rejectAbort = reject; });
  const abort = reason => { controller.abort(reason); rejectAbort(reason); };
  const cancel = () => abort(signal.reason);
  const timer = setTimeout(() => abort(Error(`${path} request timed out`)), timeout);
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    if (signal?.aborted) cancel();
    controller.signal.throwIfAborted();
    return await Promise.race([api(path, data, true, controller.signal), interrupted]);
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    interrupted.catch(() => {});
  }
}
