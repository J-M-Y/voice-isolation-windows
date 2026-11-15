// Discrete-event model, not real hardware or actual model execution.
// 240 ms blocks; 10 ms output callbacks; at most two pending input/output
// blocks plus one output block currently in progress; drop oldest on overflow.
// Input capture and output playback have separate clocks. Before playback,
// optionally wait 0/40/80/120 ms after the first processed block is queued.
// Ignore output underruns during the first 2 seconds, like audio.rs.
function run({durationMs=60000, blockMs=240, callbackMs=10, inputPpm=0, reserveMs=0, processMs}) {
  let arrival=blockMs/(1+inputPpm/1e6), callback=0, finish=Infinity;
  let pending=0, inputDrops=0, outputDrops=0, processing=0;
  let outputQueue=[], current=0, playbackStart=Infinity;
  let count=0, missingMs=0, firstMs=null;
  function startProcessing(t) {
    if (pending && finish===Infinity) { pending--; processing++; finish=t+processMs(processing); }
  }
  while (true) {
    const now=Math.min(arrival,callback,finish);
    if (now>=durationMs) break;
    if (now===finish) {
      finish=Infinity;
      if (outputQueue.length>=2) { outputQueue.shift(); outputDrops++; }
      outputQueue.push(blockMs);
      if (playbackStart===Infinity) playbackStart=now+reserveMs;
      startProcessing(now); continue;
    }
    if (now===arrival) {
      arrival+=blockMs/(1+inputPpm/1e6);
      if (pending>=2) { pending--; inputDrops++; }
      pending++; startProcessing(now); continue;
    }
    callback+=callbackMs;
    if (now<playbackStart) continue; // startup prefill, excluded from counting
    let need=callbackMs;
    while (need>1e-7) {
      if (current<1e-7) { if (outputQueue.length) current=outputQueue.shift(); else break; }
      const n=Math.min(current,need); current-=n; need-=n;
    }
    if (need>1e-7 && now>2000) { count++; missingMs+=need; if(firstMs===null)firstMs=now; }
  }
  return {starvedCallbacks:count,missingMs:+missingMs.toFixed(3),firstUnderrunMs:firstMs,inputDrops,outputDrops,processedBlocks:processing};
}
const scenarios=[
 {name:'constant 120ms',processMs:()=>120},
 {name:'100ms + 210ms every 31 blocks',processMs:i=>i%31===0?210:100},
 {name:'100ms + 340ms every 31 blocks',processMs:i=>i%31===0?340:100}
];
for(const scenario of scenarios) for(const inputPpm of [-100,0,100]) for(const reserveMs of [0,40,80,120]) {
 const result=run({inputPpm,reserveMs,processMs:scenario.processMs});
 console.log(JSON.stringify({scenario:scenario.name,inputPpm,reserveMs,...result}));
}
for(const inputPpm of [-100,100]) for(const reserveMs of [0,80,120]) {
 const result=run({durationMs:10800000,inputPpm,reserveMs,processMs:()=>120});
 console.log(JSON.stringify({scenario:'3 hours constant 120ms',inputPpm,reserveMs,...result}));
}
