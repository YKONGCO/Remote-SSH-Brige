/** Hold one file's prefetched reads until the test observes a client checkpoint. */
export function createTransferReadGate(path, offset) {
  let state = "armed";
  const pending = [];
  let markBlocked;
  const blocked = new Promise((resolve) => {
    markBlocked = resolve;
  });

  return {
    hold(request) {
      if (
        state !== "armed" ||
        request.path !== path ||
        request.offset < offset
      ) {
        return false;
      }
      pending.push(request);
      markBlocked();
      return true;
    },
    async waitUntilBlocked(timeoutMs = 10000) {
      let timer;
      try {
        await Promise.race([
          blocked,
          new Promise((_, reject) => {
            timer = setTimeout(
              () =>
                reject(new Error(`No held SFTP read for ${path} at ${offset}`)),
              timeoutMs,
            );
          }),
        ]);
      } finally {
        clearTimeout(timer);
      }
    },
    release() {
      if (state !== "armed") return;
      state = "released";
      for (const request of pending.splice(0)) request.reply();
    },
    disconnect() {
      if (state !== "armed") return;
      state = "disconnected";
      const clients = new Set(
        pending.splice(0).map((request) => request.client),
      );
      for (const client of clients) client.end();
    },
  };
}
