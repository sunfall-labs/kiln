// Transform and flush failures must reject pending readers with the original error.
for (const operation of ["transform", "flush"]) {
  for (const asynchronous of [false, true]) {
    const error = new Error(operation);
    const stream = new TransformStream({
      [operation]() {
        if (asynchronous) return Promise.reject(error);
        throw error;
      }
    });
    const reader = stream.readable.getReader();
    const writer = stream.writable.getWriter();
    const pendingRead = reader.read().catch(reason => reason === error);
    const readerClosed = reader.closed.catch(reason => reason === error);
    const writerClosed = writer.closed.catch(reason => reason === error);
    const action = operation === "transform" ? writer.write(1) : writer.close();
    console.log(operation, asynchronous,
      await action.catch(reason => reason === error),
      await pendingRead, await readerClosed, await writerClosed);
    console.log(await reader.read().catch(reason => reason === error),
      await writer.write(2).catch(reason => reason === error));
  }
}
