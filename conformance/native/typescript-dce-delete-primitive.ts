function inspect(action: () => unknown): string {
  try {
    return String(action());
  } catch (error: any) {
    return error.constructor.name;
  }
}

console.log(
  inspect(() => delete ("text" as any).length),
  delete (5 as any).missing,
  delete ({} as any).missing,
);
