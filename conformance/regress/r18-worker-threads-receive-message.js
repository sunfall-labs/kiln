// `receiveMessageOnPort` answered `undefined` for every port, whatever it held, so a program that
// polls a port -- the synchronous half of the worker_threads messaging API -- saw an empty queue
// forever and never said why. It takes the oldest queued message, wrapped, and leaves the rest.
const { MessageChannel, receiveMessageOnPort } = require('worker_threads');

const { port1, port2 } = new MessageChannel();
port1.postMessage({ n: 1 });
port1.postMessage('two');

const first = receiveMessageOnPort(port2);
console.log('first', JSON.stringify(first));
console.log('second', JSON.stringify(receiveMessageOnPort(port2)));
console.log('empty', receiveMessageOnPort(port2));

// The message is a copy: changing what was sent does not change what is received.
const sent = { list: [1, 2] };
port1.postMessage(sent);
sent.list.push(3);
console.log('copied', JSON.stringify(receiveMessageOnPort(port2).message));

try {
	receiveMessageOnPort({});
} catch (error) {
	console.log('not a port', error.code, error.message);
}

port1.close();
