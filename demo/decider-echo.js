// A stand-in for an LLM decider (like `claude -p`). Reads the terminal tail
// from stdin and prints one directive. Real usage: puppetty --decider "claude -p" ...
let input = '';
process.stdin.on('data', (d) => (input += d));
process.stdin.on('end', () => {
  if (/project name/i.test(input)) {
    console.log('SEND:my-cool-app');
  } else if (/password/i.test(input)) {
    console.log('CANCEL');
  } else {
    console.log('WAIT');
  }
});
