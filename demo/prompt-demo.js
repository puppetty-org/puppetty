// Simulates a script that blocks on interactive prompts.
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
const ask = (q) => new Promise((res) => rl.question(q, res));
const sleep = (ms) => new Promise((res) => setTimeout(res, ms));

console.log('Preparing installation...');
await sleep(1_500);

const a1 = await ask('Do you want to continue? [y/N] ');
console.log(`  -> got: ${JSON.stringify(a1)}`);
if (!/^y/i.test(a1)) { console.log('Aborted.'); process.exit(1); }

console.log('Installing (simulated)...');
await sleep(2_000);

const a2 = await ask('Enable color output? (yes/no): ');
console.log(`  -> got: ${JSON.stringify(a2)}`);

await ask('Press Enter to finish...');
console.log('Done! All prompts were answered.');
rl.close();
process.exit(0);
