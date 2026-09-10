export function output() {
  const text = { out: '', error: '' };
  return { text, io: { writeOut: (value: string) => { text.out += value; },
    writeError: (value: string) => { text.error += value; } } };
}
