// Simulates a password prompt — puppetty must NOT answer it, and should
// cancel after --prompt-timeout.
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
rl.question('Enter your password: ', (pw) => {
  console.log(`SECURITY FAILURE: something typed a password (${pw.length} chars)!`);
  rl.close();
  process.exit(1);
});
