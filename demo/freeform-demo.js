// Simulates a script with a free-text prompt no built-in rule can answer —
// exercises the --decider path.
import readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
const ask = (q) => new Promise((res) => rl.question(q, res));

const name = await ask('Project name: ');
console.log(`Creating project ${JSON.stringify(name)}...`);
console.log('Done.');
rl.close();
process.exit(0);
