/** Local checks at the buffered rewrite boundary, before a result is cached or
 * edited. These recognize explicit EN/ZH clocks, calendar anchors and numeric quantities, not
 * general meaning. Uncertain matching keeps the current input recoverable.
 * No model, provider request, old original, or later manual edit is involved.
 */
type Span = { start: number; end: number };
type Period = 'am' | 'pm' | '';
type Bound = '' | 'upper' | 'under' | 'lower' | 'over' | 'exact' | 'approx';
type Qualifier = Span & { am?: string; pm?: string };
type Clock = Span & { key: string; period: Period; relation: string; negated: boolean; qualifier?: Qualifier;
  insertAt: number; language: 'en' | 'zh'; implicitPeriod: boolean; rawHour: number; rangeStart: boolean };
type Quantity = Span & { key: string; family: string; bound: Bound; qualifiers: Span[]; genericCounter?: boolean };
type Edit = Span & { value: string };

export interface BufferedFactReview { outputText: string; correctedFacts: number }
export class BufferedSourceFactError extends Error {
  constructor() { super('buffered_source_facts_unmatched'); }
}
function fail(): never { throw new BufferedSourceFactError(); }
const cnDigits: Record<string, number> = { 零:0, 〇:0, 一:1, 二:2, 两:2, 兩:2, 三:3, 四:4, 五:5, 六:6, 七:7, 八:8, 九:9 };
const enNumbers = ['zero','one','two','three','four','five','six','seven','eight','nine','ten','eleven','twelve','thirteen','fourteen','fifteen','sixteen','seventeen','eighteen','nineteen'];
const enTens = ['twenty','thirty','forty','fifty','sixty','seventy','eighty','ninety'];
const wordNumber = `(?:${[...enNumbers, ...enTens].join('|')})(?:[- ](?:one|two|three|four|five|six|seven|eight|nine))?`;
const hundreds = `(?:(?:one|two|three|four|five|six|seven|eight|nine)[ -]+hundred(?:[ -]+(?:and[ -]+)?${wordNumber})?|${wordNumber})`;
const quantityWordNumber = `(?:${hundreds}[ -]+thousand(?:[ -]+(?:and[ -]+)?${hundreds})?|${hundreds})`;
const cnNumber = '[零〇一二两兩三四五六七八九十百千萬万]+';
const cnClockNumber = '(?:[一二两兩三四五]十[一二三四五六七八九]?|十[一二三四五六七八九]?|[零〇一二两兩三四五六七八九])';
const numberPattern = `(?:\\d+(?:,\\d{3})*(?:\\.\\d+)?|${quantityWordNumber}|${cnNumber})`;
function numeral(raw: string): number {
  const value = raw.toLowerCase().replace(/,/g, '');
  if (/^\d+(?:\.\d+)?$/.test(value)) return Number(value);
  if (/^[零〇一二两兩三四五六七八九]+$/.test(value) && value.length>1) return NaN;
  const parts = value.split(/[- ]/);
  if (parts.every(part => enNumbers.includes(part) || enTens.includes(part) || ['and','hundred','thousand'].includes(part))) {
    let total=0,section=0;
    for(const part of parts) {
      if(part==='hundred')section=(section||1)*100;
      else if(part==='thousand'){total+=section*1000;section=0;}
      else if(part!=='and')section+=enNumbers.includes(part)?enNumbers.indexOf(part):(enTens.indexOf(part)+2)*10;
    }
    return total+section;
  }
  let total = 0, section = 0, digit = 0;
  for (const ch of value) {
    if (ch in cnDigits) digit = cnDigits[ch];
    else if ('十百千'.includes(ch)) { section += (digit || 1) * ({ 十:10, 百:100, 千:1000 }[ch] ?? 0); digit = 0; }
    else if (ch === '万' || ch === '萬') { total += (section + digit || 1) * 10000; section = digit = 0; }
    else return NaN;
  }
  return total + section + digit;
}
function quantityValue(raw:string): string | null {
  if(/^\d/.test(raw)) {
    const [whole,decimal='']=raw.replace(/,/g,'').split('.');
    const fraction=decimal.replace(/0+$/,'');
    return (whole.replace(/^0+(?=\d)/,'')||'0')+(fraction?'.'+fraction:'');
  }
  const value=numeral(raw);
  return Number.isSafeInteger(value)?String(value):null;
}
const clockContext = /\b(?:meet|meeting|call|arrive|leave|start|end|appointment|schedule|deadline|today|tomorrow|tonight|morning|afternoon|evening|monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tue|tues|wed|thu|thur|thurs|fri|sat|sun)\b|见|見|会|會|约|約|时间|時間|到达|到達|出发|出發|开|開|截止|今天|明天|星期|周[一二三四五六日天]|週/iu;
const quantityAfter = /^\s*(?:dollars?|USD|EUR|items?|pieces?|percent|km|miles?|kg|minutes?|hours?|days?|weeks?|years?)\b|^\s*[%$€]|^\s*个|^\s*項/iu;
function clause(text: string, start: number, end: number): string {
  return text.slice(0,start).split(/[。！？!?;；\n]/).slice(-1)[0] + text.slice(end).split(/[。！？!?;；\n]/)[0];
}
function negatedClock(text:string,start:number,end:number,language:'en'|'zh'): boolean {
  const prefix=text.slice(0,start).split(/[。！？!?.,，;；\r\n]/).slice(-1)[0] ?? '';
  const suffix=text.slice(end).split(/[。！？!?.,，;；\r\n]/)[0];
  return language==='en' ? /\b(?:not|never|cannot|can[’']t|don[’']t|mustn[’']t)\b/iu.test(prefix)
    : /不要|不能|不得|不许|不許|禁止|没有|沒有|未|不/u.test(prefix + (/^\s*(?:不要|不能|不得|不许|不許|禁止)/u.test(suffix)?suffix:''));
}
function periodValue(raw: string): Period {
  return /^(?:凌晨|清晨|早上|上午|a\.?\s*m|in the morning|morning)/i.test(raw) ? 'am' : 'pm';
}
function relationValue(raw: string): string {
  const value = raw.toLowerCase().replace(/\s+/g,' ').trim();
  if (/^(?:不早于|不早於|no earlier than|not before)$/.test(value)) return 'not_before';
  if (/^(?:不晚于|不晚於|最迟|最遲|no later than|not after)$/.test(value)) return 'by';
  if (/^(?:之前|以前|前|before|by)$/.test(value)) return 'by';
  if (/^(?:之后|之後|以后|以後|后|後|after)$/.test(value)) return 'after';
  return '';
}
function clocks(text: string): Clock[] {
  const found: Clock[] = [];
  const zh = new RegExp(`(?<!\\d)(?:(不早于|不早於|不晚于|不晚於|最迟|最遲)\\s*)?(?:(凌晨|清晨|早上|上午|中午|下午|傍晚|晚上|晚间|晚間)[ \\t]*)?(${cnClockNumber}|\\d{1,2})[ \\t]*(?:[点點时時](半|(?:${cnClockNumber}|\\d{1,2})[ \\t]*分?)?|:([0-5]\\d))[ \\t]*(之前|以前|之后|之後|以后|以後|前|后|後)?(?![\\d小时小時])`, 'gu');
  for (const match of text.matchAll(zh)) {
    const hour = numeral(match[3]), minute = match[4] === '半' ? 30 : numeral((match[4] ?? match[5] ?? '0').replace(/分$/,''));
    if (!(hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59)) continue;
    if(match[5] && !match[1] && !match[2] && !match[6]) continue;
    if (!match[1] && !match[2] && !match[4] && !match[5] && !match[6] && !/[时時]/u.test(match[0])) {
      const before=text.slice(0,match.index!),after=text.slice(match.index!+match[0].length);
      const standalone=!before.trim() && /^[。.!\s]*$/u.test(after);
      const localBefore=before.split(/[。！？!?.,，;；\r\n]/).slice(-1)[0];
      const contextualPoint=/(?:星期[一二三四五六日天]|[周週][一二三四五六日天]|今天|明天|后天|後天|会议|會議|演示|预约|預約|闹钟|鬧鐘|日程)/u.test(localBefore)
        && /^\s*(?:[，,。.;；！？!?]|$)/u.test(after);
      const previous=found[found.length-1];
      const correction=previous && /^\s*[,，]\s*(?:而?不是|并非|並非)\s*$/u.test(text.slice(previous.end,match.index!))
        && /^\s*(?:[，,。.;；！？!?]|$)/u.test(after);
      const timeBefore=contextualPoint || correction || /(?:星期[一二三四五六日天]|[周週][一二三四五六日天]|今天|明天|后天|後天|时间|時間|时刻|時刻|约在?|約在?|定于|定於|安排在|安排於)(?:的)?[：: \t]*$/u.test(before);
      const timeAfter=/^\s*(?:见|見|开会|開會|开始|開始|结束|結束|到达|到達|出发|出發|抵达|抵達|来|來|举行|舉行)/u.test(after);
      if(!standalone && !timeBefore && !timeAfter)continue;
    }
    if (match[2] === '中午' && hour !== 12) fail();
    const period = match[2] ? periodValue(match[2]) : hour > 12 ? 'pm' : hour === 0 ? 'am' : '';
    const relativeStart = match[2] ? match[0].indexOf(match[2]) : -1;
    const numberStart = match.index! + match[0].indexOf(match[3], relativeStart < 0 ? 0 : relativeStart + match[2].length);
    found.push({start:match.index!,end:match.index!+match[0].length,key:`${hour%12}:${minute}`,period,
      relation:relationValue(match[1] || match[6] || ''),negated:negatedClock(text,match.index!,match.index!+match[0].length,'zh'),language:'zh',rawHour:hour,rangeStart:false,
      qualifier:match[2] ? {start:match.index!+relativeStart,end:match.index!+relativeStart+match[2].length,am:'上午',pm:'下午'} : undefined,
      insertAt:numberStart,implicitPeriod:!match[2] && (hour===0 || hour>12)});
  }
  const en = new RegExp(`(?<![\\w:.])(?:(no\\s+(?:earlier|later)\\s+than|not\\s+(?:before|after)|before|after|by|at|from|between|to)\\s+)?(\\d{1,2}|${wordNumber})(?::([0-5]\\d)|\\.([0-5]\\d)(?=\\s*[ap]\\.?\\s*m))?(?:([ \\t]*)([ap]\\.?\\s*m\\.?|in\\s+the\\s+(?:morning|afternoon|evening)|at\\s+night|o[’']clock))?(?![\\w:]|\\.\\d)`, 'giu');
  for (const match of text.matchAll(en)) {
    if (found.some(old=>match.index!<old.end && old.start<match.index!+match[0].length)) continue;
    const hour = numeral(match[2]), minute = numeral(match[3] ?? match[4] ?? '0');
    if (!(hour >= 0 && hour <= 23)) continue;
    const end = match.index!+match[0].length, rawPeriod = match[6] && !/^o[’']clock$/i.test(match[6]) ? match[6] : '';
    if(!match[3]&&!match[4]&&!match[6]&&/^\s*(?:January|February|March|April|May|June|July|August|September|October|November|December|Jan\.?|Feb\.?|Mar\.?|Apr\.?|Jun\.?|Jul\.?|Aug\.?|Sep(?:t)?\.?|Oct\.?|Nov\.?|Dec\.?)\b/i.test(text.slice(end)))continue;
    if(!rawPeriod && (/\b(?:ratio|scale|proportion)\s*(?:of|=|:)?\s*$|(?:比例|比率)\s*(?:为|為|是|:|：)?\s*$/iu.test(text.slice(0,match.index!))
      || /^\s*(?:ratio|scale|比例|比率)(?:\b|[。,.，])/iu.test(text.slice(end))))continue;
    const previous = found.filter(old=>old.language==='en' && old.end<=match.index!).slice(-1)[0];
    const rangeContinuation = previous && previous.rangeStart && /^\s*(?:and|to|[-–—])\s*$/i.test(text.slice(previous.end,match.index!));
    const correction=previous && /^\s*[,;]?\s*(?:not|rather than|instead of)\s*$/i.test(text.slice(previous.end,match.index!));
    if (!match[3] && !match[4] && !match[6] && !correction && !((/^(?:at|before|after|by|from|between|to)$/i.test(match[1] ?? '') || rangeContinuation) && clockContext.test(clause(text,match.index!,end)))) continue;
    if (quantityAfter.test(text.slice(end))) continue;
    let period: Period = rawPeriod ? periodValue(rawPeriod) : hour > 12 ? 'pm' : hour===0 ? 'am' : '';
    const partStart = rawPeriod ? match.index! + match[0].lastIndexOf(rawPeriod) : -1;
    let qualifier: Qualifier | undefined = rawPeriod ? {start:partStart-(match[5]?.length ?? 0),end,am:(match[5] ?? '')+'a.m.',pm:(match[5] ?? '')+'p.m.'} : undefined;
    if (!period) {
      const prefix = /\b(morning|afternoon|evening)[ \t]+(?:at[ \t]+)?$/i.exec(text.slice(0,match.index!));
      if (prefix) { period=periodValue(prefix[1]); qualifier={start:prefix.index,end:prefix.index+prefix[1].length+1,am:'morning ',pm:'afternoon '}; }
    }
    const cjkContext=/\p{Script=Han}/u.test(clause(text,match.index!,end));
    const clock: Clock = {start:match.index!,end,key:`${hour%12}:${minute}`,period,relation:relationValue(match[1] ?? ''),negated:negatedClock(text,match.index!,end,cjkContext?'zh':'en'),
      language:cjkContext?'zh':'en',rawHour:hour,rangeStart:/^(?:from|between)$/i.test(match[1] ?? ''),qualifier,
      insertAt:cjkContext?match.index!+match[0].indexOf(match[2]):end,implicitPeriod:!rawPeriod && !qualifier && (hour===0||hour>12)};
    if (rangeContinuation && previous) {
      if (previous.rawHour > hour && !!previous.period !== !!clock.period) fail();
      if (!previous.period && clock.period) { previous.period=clock.period; previous.qualifier=clock.qualifier; }
      if (!clock.period && previous.period) clock.period=previous.period;
    }
    found.push(clock);
  }
  return found.sort((a,b)=>a.start-b.start);
}
const unitAliases: [RegExp,string][] = [
  [/^(?:USD|US\$|\$|U\.?S\.? dollars?|dollars?|美元|美金)$/iu,'USD'],[/^(?:EUR|€|euros?|欧元|歐元)$/iu,'EUR'],
  [/^(?:CNY|RMB|¥|人民币|人民幣|元)$/iu,'CNY'],[/^(?:GBP|£|pounds?|英镑|英鎊)$/iu,'GBP'],
  [/^(?:%|percent|百分之)$/iu,'percent'],[/^(?:people|persons?|attendees?|participants?|人|位)$/iu,'people'],
  [/^(?:items?|pieces?|units?|个|個|件|项|項)$/iu,'items'],[/^(?:kg|kilograms?|公斤|千克)$/iu,'kg'],
  [/^(?:km|kilomet(?:er|re)s?|公里|千米)$/iu,'km'],
  [/^(?:seconds?|秒钟|秒鐘|秒)$/iu,'seconds'],[/^(?:minutes?|分钟|分鐘)$/iu,'minutes'],
  [/^(?:hours?|小时|小時)$/iu,'hours'],[/^(?:days?|天)$/iu,'days'],[/^(?:weeks?|周|週)$/iu,'weeks'],
];
const unitPattern = '(?:U\\.?S\\.? dollars?|USD|US\\$|dollars?|美元|美金|EUR|euros?|欧元|歐元|CNY|RMB|人民币|人民幣|GBP|pounds?|英镑|英鎊|percent|%|people|persons?|attendees?|participants?|items?|pieces?|units?|kilograms?|kg|kilomet(?:er|re)s?|km|seconds?|minutes?|hours?|days?|weeks?|秒钟|秒鐘|秒|分钟|分鐘|小时|小時|天|周|週|公斤|千克|公里|千米|个|個|件|项|項|人|位|元)';
function bounds(text: string, start: number, end: number): {bound:Bound;qualifiers:Span[]} {
  const candidates: {bound:Bound;span:Span}[]=[];
  const patterns: [Bound,string][] = [
    ['upper','不能超过|不能超過|不要超过|不要超過|不超过|不超過|至多|最多|no more than|not more than|at most|up to|within'],
    ['under','少于|少於|低于|低於|less than|under|below'],
    ['lower','不能低于|不能低於|不要低于|不要低於|不低于|不低於|不少于|不少於|至少|no less than|not less than|at least'],
    ['over','超过|超過|多于|多於|more than|over|above'],
    ['exact','恰好|正好|exactly|precisely'],['approx','大约|大約|约|約|about|approximately|around'],
  ];
  const before=text.slice(0,start);
  for (const [bound,pattern] of patterns) {
    const match=new RegExp(`(?<![A-Za-z])(?:${pattern})[ \\t]*$`,'iu').exec(before);
    if (match) candidates.push({bound,span:{start:match.index,end:start}});
  }
  const suffix=/^[ \t]*(以内|以內|以下|以上|左右)/u.exec(text.slice(end));
  if (suffix) candidates.push({bound:/以内|以內|以下/.test(suffix[1])?'upper':suffix[1]==='以上'?'lower':'approx',span:{start:end,end:end+suffix[0].length}});
  // A longer negated comparison owns a nested shorter token, e.g. 不超过.
  const maximal=candidates.filter(x=>!candidates.some(y=>y!==x && y.span.start<x.span.start && y.span.end===x.span.end));
  if (new Set(maximal.map(x=>x.bound)).size>1) fail();
  return {bound:maximal[0]?.bound ?? '',qualifiers:maximal.map(x=>x.span)};
}
function quantities(text: string): Quantity[] {
  const found: Quantity[]=[];
  const suffix=new RegExp(`(?<![A-Za-z\\d.,])(${numberPattern})[ \\t]*(${unitPattern})(?![A-Za-z])`,'giu');
  const prefix=new RegExp(`(US\\$|USD|EUR|CNY|RMB|GBP|[$€¥£])[ \\t]*(${numberPattern})(?:[ \\t]*(${unitPattern}))?(?![A-Za-z\\d,]|\\.\\d)`,'giu');
  for (const [pattern,numberIndex,unitIndex] of [[suffix,1,2],[prefix,2,1]] as const) {
    for (const match of text.matchAll(pattern)) {
      const start=match.index!;let end=start+match[0].length;
      if(pattern===suffix && /第\s*$/u.test(text.slice(0,start)))continue;
      const family=unitAliases.find(([pattern])=>pattern.test(match[unitIndex]))?.[1];
      const value=quantityValue(match[numberIndex]);
      if (!family || value===null) continue;
      if(pattern===prefix) {
        if(match[3] && unitAliases.find(([pattern])=>pattern.test(match[3]))?.[1]!==family)fail();
        const trailing=new RegExp(`^[ \\t]*(${unitPattern})(?![A-Za-z])`,'iu').exec(text.slice(end));
        if(trailing) {
          const unit=unitAliases.find(([pattern])=>pattern.test(trailing[1]))?.[1];
          if(unit!==family)fail();
          end+=trailing[0].length;
        }
      }
      const key=`${family}:${value}`;
      const overlap=found.find(old=>start<old.end && old.start<end);
      if(overlap) {
        if(overlap.key!==key)fail();
        overlap.start=Math.min(start,overlap.start);overlap.end=Math.max(end,overlap.end);
        Object.assign(overlap,bounds(text,overlap.start,overlap.end));
      } else found.push({start,end,key,family,genericCounter:/^(?:个|個)$/u.test(match[unitIndex]),...bounds(text,start,end)});
    }
  }
  return found.sort((a,b)=>a.start-b.start);
}
/** A translated count can name the object directly (one/a button), without
 * translating the Chinese classifier as "one item". Match only explicit
 * count counterparts; money, duration, dates and clocks never supply them.
 * Ambiguous matches fail rather than choosing a noun or changing a number.
 */
function translatedQuantities(source:string,target:string): [Quantity[],Quantity[]] {
  const left=quantities(source),right=quantities(target);
  const count=(q:Quantity)=>q.family==='items'||q.family==='people';
  const value=(q:Quantity)=>q.key.slice(q.key.indexOf(':')+1);
  for(const [items,others] of [[left,right],[right,left]])for(const item of items)if(item.genericCounter) {
    const families=new Set(others.filter(q=>count(q)&&value(q)===value(item)).map(q=>q.family));
    if(families.size>1)fail();
    if(families.size===1){item.family=[...families][0];item.key=item.family+':'+value(item);}
  }
  for(const [text,items,others] of [[target,right,left],[source,left,right]] as const) {
    const clockSpans=clocks(text);
    for(const [key,wanted] of groups(others.filter(count))) {
      const missing=wanted.length-items.filter(q=>q.key===key).length;
      if(missing<=0)continue;
      const number=value(wanted[0]);
      const candidates:Quantity[]=[];
      const pattern=new RegExp(`(?<![A-Za-z\\d])(${numberPattern})(?![A-Za-z\\d])`,'giu');
      const collect=(start:number,end:number,raw:string)=>{
        if(quantityValue(raw)!==number || [...items,...clockSpans].some(q=>start<q.end&&q.start<end))return;
        const after=text.slice(end);
        if(!/^[ -]+[A-Za-z]/u.test(after))return;
        if(/\b(?:January|February|March|April|May|June|July|August|September|October|November|December)[ \t]+$/iu.test(text.slice(0,start)))return;
        const next=/^[ -]+([A-Za-z]+)/u.exec(after)?.[1]??'';
        const known=unitAliases.find(([unit])=>unit.test(next))?.[1];
        if(known && known!==wanted[0].family)return;
        candidates.push({start,end,key,family:wanted[0].family,...bounds(text,start,end)});
      };
      for(const match of text.matchAll(pattern))collect(match.index!,match.index!+match[0].length,match[1]);
      if(!candidates.length && number==='1')for(const match of text.matchAll(/\b(?:a|an)(?=[ \t]+(?!few\b|lot\b|couple\b|number\b|bit\b|half\b|quarter\b)[A-Za-z])/giu)) {
        collect(match.index!,match.index!+match[0].length,'1');
      }
      if(candidates.length!==missing)fail();
      items.push(...candidates);
    }
  }
  return [left,right];
}
function groups<T extends {key:string}>(items:T[]): Map<string,T[]> {
  const map=new Map<string,T[]>();
  for (const item of items) map.set(item.key,[...(map.get(item.key)??[]),item]);
  return map;
}
function calendarFacts(text:string): string[] {
  const facts:string[]=[];
  const weekdays=['mon','tue','wed','thu','fri','sat','sun'];
  for(const match of text.matchAll(/\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tues?|wed|thu(?:rs?)?|fri|sat|sun)\b|(?:星期|周|週)([一二三四五六日天])/giu)) {
    if(match[1] && match[1].length<6 && match[1][0]===match[1][0].toLowerCase()
      && !clockContext.test(clause(text,match.index!,match.index!+match[0].length)))continue;
    facts.push('weekday:'+(match[1]?weekdays.indexOf(match[1].toLowerCase().slice(0,3)):'一二三四五六日'.indexOf(match[2]==='天'?'日':match[2])));
  }
  const monthNames=['january','february','march','april','may','june','july','august','september','october','november','december'];
  const monthPattern='(?:January|February|March|April|May|June|July|August|September|October|November|December|Jan\\.?|Feb\\.?|Mar\\.?|Apr\\.?|Jun\\.?|Jul\\.?|Aug\\.?|Sep(?:t)?\\.?|Oct\\.?|Nov\\.?|Dec\\.?)';
  const monthIndex=(raw:string)=>monthNames.findIndex(name=>name.startsWith(raw.toLowerCase().replace(/\.$/,'')))+1;
  for(const match of text.matchAll(new RegExp(`\\b(${monthPattern})[ \\t]+(\\d{1,2})(?:st|nd|rd|th)?\\b|\\b(\\d{1,2})(?:st|nd|rd|th)?[ \\t]+(${monthPattern})\\b`,'giu'))) {
    const month=monthIndex(match[1]??match[4]),day=Number(match[2]??match[3]);
    if(month>0 && day>=1 && day<=31) facts.push(`date:${month}:${day}`);
  }
  for(const match of text.matchAll(new RegExp(`(${cnClockNumber}|\\d{1,2})月[ \\t]*(${cnClockNumber}|\\d{1,2})[日号號]`,'gu'))) {
    const month=numeral(match[1]),day=numeral(match[2]);
    if(month>=1&&month<=12&&day>=1&&day<=31)facts.push(`date:${month}:${day}`);
  }
  for(const match of text.matchAll(/\b(?:1[5-9]\d{2}|20\d{2}|21\d{2})[-/](\d{1,2})[-/](\d{1,2})\b/g)) {
    const month=Number(match[1]),day=Number(match[2]);
    if(month>=1&&month<=12&&day>=1&&day<=31)facts.push(`date:${month}:${day}`);
  }
  for(const match of text.matchAll(/(?<!\d)(?:1[5-9]\d{2}|20\d{2}|21\d{2})(?!\d)/g)) {
    const start=match.index!,end=start+match[0].length;
    if(quantityAfter.test(text.slice(end))||/[$€¥£]\s*$/.test(text.slice(0,start)))continue;
    if(/^年|^[-/]\d{1,2}[-/]\d{1,2}/u.test(text.slice(end))
      ||new RegExp(`\\b(?:${monthPattern}(?:[ \\t]+\\d{1,2}(?:st|nd|rd|th)?)?|year)[, \\t]+$`,'i').test(text.slice(0,start)))facts.push('year:'+match[0]);
  }
  return facts;
}
function addEdit(edits:Edit[], edit:Edit): void {
  const same=edits.find(old=>old.start===edit.start && old.end===edit.end);
  if (same) { if(same.value!==edit.value) fail(); return; }
  if(edits.some(old=>edit.start<old.end && old.start<edit.end)) fail();
  edits.push(edit);
}
function preserveFacts(source: string, outputText: string, translation:boolean): BufferedFactReview {
  if (source===outputText) return {outputText,correctedFacts:0};
  const edits:Edit[]=[];
  const sourceClocks=groups(clocks(source)),targetClocks=groups(clocks(outputText));
  if(sourceClocks.size!==targetClocks.size) fail();
  if(sourceClocks.size) {
    for(const [key,originals] of sourceClocks) {
      const targets=targetClocks.get(key);
      if(!targets || targets.length!==originals.length || new Set(originals.map(x=>x.period+'|'+x.relation+'|'+x.negated)).size!==1) fail();
      const original=originals[0];
      for(const target of targets) {
        if(original.relation!==target.relation || original.negated!==target.negated) fail();
        if(original.period===target.period) continue;
        if(target.implicitPeriod) fail();
        if(target.qualifier) {
          // AM./p.m. can also own the sentence's final dot. Removing an
          // unsupported daypart must not join sentences or remove that stop.
          const keepStop=/\.$/.test(outputText.slice(target.qualifier.start,target.qualifier.end))
            && /^(?:\s*$|\s+[A-Z])/u.test(outputText.slice(target.qualifier.end));
          addEdit(edits,{...target.qualifier,value:original.period ? target.qualifier[original.period]! : keepStop?'.':''});
        }
        else if(original.period) addEdit(edits,{start:target.insertAt,end:target.insertAt,
          value:target.language==='zh' ? original.period==='am'?'上午':'下午' : original.period==='am'?' a.m.':' p.m.'});
      }
    }
  }
  const [sourceNumbers,targetNumbers]=translation?translatedQuantities(source,outputText):[quantities(source),quantities(outputText)];
  const sourceQuantities=groups(sourceNumbers),targetQuantities=groups(targetNumbers);
  for(const [key,originals] of sourceQuantities) {
    const targets=targetQuantities.get(key);
    if(!targets || targets.length!==originals.length || new Set(originals.map(x=>x.bound)).size!==1) fail();
    const original=originals[0];
    for(const target of targets) {
      if(original.bound===target.bound) continue;
      if(original.bound || !target.qualifiers.length) fail();
      for(const qualifier of target.qualifiers) addEdit(edits,{...qualifier,value:''});
    }
  }
  for(const key of targetQuantities.keys()) if(!sourceQuantities.has(key)) fail();
  const sourceCalendar=calendarFacts(source),targetCalendar=calendarFacts(outputText);
  for(const category of ['weekday:','date:','year:']) {
    const wanted=sourceCalendar.filter(x=>x.startsWith(category)).sort();
    const actual=targetCalendar.filter(x=>x.startsWith(category)).sort();
    if(wanted.length!==actual.length || wanted.some((x,index)=>x!==actual[index]))fail();
  }
  const corrected=edits.sort((a,b)=>b.start-a.start).reduce((text,edit)=>text.slice(0,edit.start)+edit.value+text.slice(edit.end),outputText);
  return {outputText:corrected,correctedFacts:edits.length};
}
export function preserveBufferedSourceFacts(source:string,output:string):BufferedFactReview {
  return preserveFacts(source,output,false);
}
export function preserveTranslatedSourceFacts(source:string,output:string):BufferedFactReview {
  return preserveFacts(source,output,true);
}
