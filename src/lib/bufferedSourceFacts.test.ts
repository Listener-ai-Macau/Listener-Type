import assert from 'node:assert/strict';
import { BufferedSourceFactError, preserveBufferedSourceFacts as review, preserveTranslatedSourceFacts as translate } from './bufferedSourceFacts.ts';

const source='嗨,萨姆,\n\n星期四三点见吧 午餐前我会把笔记寄出去 请将预算维持在50美元,如果需要改变时间,请告诉我.';
const service='Hi Sam,\n\n周四下午三点见。午餐前我会把笔记寄出。请将预算控制在 50 美元以内。如需调整时间，请告知我。';
const actual=review(source,service);
assert.equal(actual.outputText,service.replace('下午','').replace('以内',''));
assert.equal(actual.correctedFacts,2);
assert.ok(!actual.outputText.includes('恰好') && !actual.outputText.includes('exactly'));

const cases:[string,string,string][]=[
 ['周四三点见。预算保持在75欧元。','周四下午三点见。预算维持75欧元以内。','周四三点见。预算维持75欧元。'],
 ['下午三点见。','三点见。','下午三点见。'],
 ['星期四三点见。','星期四下午三点见。','星期四三点见。'],
 ['Meet at 3 tomorrow.','Meet at 3 p.m. tomorrow.','Meet at 3 tomorrow.'],
 ['Meet at three tomorrow.','Meet at 3 p.m. tomorrow.','Meet at 3 tomorrow.'],
 ['Meet at 3 pm tomorrow.','Meet at 3 tomorrow.','Meet at 3 p.m. tomorrow.'],
 ['Meet between 10 and 11 tomorrow.','Meet between 10 and 11 p.m. tomorrow.','Meet between 10 and 11 tomorrow.'],
 ['预算保持50美元。','预算保持五十美元以内。','预算保持五十美元。'],
 ['Keep 12 attendees.','Keep twelve participants.','Keep twelve participants.'],
 ['Keep $50.','Retain 50 dollars.','Retain 50 dollars.'],
 ['Keep one hundred and fifty dollars.','Retain $150.','Retain $150.'],
 ['Keep $1.10.','Retain 1.1 dollars.','Retain 1.1 dollars.'],
 ['Keep $123456789123456789.','Retain 123456789123456789 dollars.','Retain 123456789123456789 dollars.'],
 ['Keep $50USD.','Retain 50 dollars.','Retain 50 dollars.'],
 ['Keep at most USD 50 dollars.','Retain at most $50.','Retain at most $50.'],
 ['Keep 12 attendees.','Keep twelve participants以内。','Keep twelve participants。'],
 ['预算不超过50美元。','预算最多50美元。','预算最多50美元。'],
 ['预算不能低于50美元。','预算至少50美元。','预算至少50美元。'],
 ['Meet after 3 pm. Do not bring a laptop.','下午三点之后见，不要带电脑。','下午三点之后见，不要带电脑。'],
 ['Meet on November 9, 2026 at 3 pm.','2026年11月9日下午三点见。','2026年11月9日下午三点见。'],
 ['Schedule the meeting before 9 November 2026.','会议安排在2026年11月9日之前。','会议安排在2026年11月9日之前。'],
 ['Use a ratio of 1:50.','Use a ratio of 1 to 50.','Use a ratio of 1 to 50.'],
 ['星期四三点见。','Thursday at 3.','Thursday at 3.'],
 ['Friday at 2.','Fri at 2.','Fri at 2.'],
 ['请给我三点建议。','请给我三条建议。','请给我三条建议。'],
 ['我有三点建议，明天的会议需要讨论。','明天的会议需要讨论以下三条建议。','明天的会议需要讨论以下三条建议。'],
 ['Keep $50.\n\nMeet at 3 tomorrow.','Keep $50.\n\nMeet at 3 tomorrow.','Keep $50.\n\nMeet at 3 tomorrow.'],
];
for(const [input,returned,expected] of cases) assert.equal(review(input,returned).outputText,expected,input);

for(const [input,returned] of [
 ['周四三点见。','周四四点见。'],['Meet at 3 tomorrow.','Meet at 15:00 tomorrow.'],
 ['Meet at 3 tomorrow.','We will meet tomorrow.'],
 ['Meet at 3 tomorrow and 3 pm Friday.','Meet at 3 pm tomorrow and 3 pm Friday.'],
 ['预算保持50美元。','预算保持60美元。'],['预算保持50美元。','预算已确认。'],
 ['预算不超过50美元。','预算至少50美元。'],['预算不能低于50美元。','预算最多50美元。'],
 ['预算恰好50美元。','预算保持50美元。'],['预算约50美元。','预算50美元以内。'],
 ['预算50美元，另一个预算不超过50美元。','两个预算最多50美元。'],
 ['Meet after 3 pm tomorrow.','Meet before 3 pm tomorrow.'],
 ['Keep 12 attendees.','Keep 11 attendees.'],
 ['Keep 12 attendees and 4 items.','Keep 12 attendees.'],
 ['预算保持50美元。','预算保持50欧元。'],
 ['Do not meet before 3 pm tomorrow.','Meet before 3 pm tomorrow.'],
 ['三点之前不要开会。','三点之前开会。'],
 ['Meet on November 9, 2026.','2025年11月9日见。'],
 ['Meet on November 9, 2026.','11月9日见。'],
 ['Meet on November 9, 2026.','2026年11月10日见。'],
 ['星期四三点见。','星期五三点见。'],
 ['Keep $50.','Retain 60 dollars.'],
 ['Keep one hundred and fifty dollars.','Retain $50.'],
 ['Keep at most USD 50 dollars.','Retain at least $50.'],
 ['Keep $123456789123456789.','Retain 123456789123456788 dollars.'],
 ['下午三点见。','周四三点见。'],
 ['The budget is approved.','The budget is $50.'],
 ['We will meet.','We will meet at 3 p.m.'],
]) assert.throws(()=>review(input,returned),BufferedSourceFactError,input);

const projectSource='团队项目更新：今天上午已完成登录页文案和错误提示修改，测试环境验证通过。支付页仍有一个手机端按钮错位，预计明天下午三点前修好。请大家先不要发布，等我确认后再合并。另外，周五客户演示仍是十点，不是十一点。';
const projectReply="Team project update: This morning we finished revising the login page copy and error messages, and verification passed in the test environment. The payment page still has one misaligned button on mobile, which is expected to be fixed by 3 PM tomorrow. Please don't publish yet; wait for my confirmation before merging. Also, Friday's client demo is still at 10 AM, not 11 AM.";
const corrected=projectReply.replace('10 AM','10').replace('11 AM','11');
assert.equal(translate(projectSource,projectReply).outputText,corrected);
assert.equal(translate(projectSource,corrected).outputText,corrected,'corrected text is stable');
for(const [input,returned,expected] of [
 ['周五客户演示仍是十点，不是十一点。',"Friday's client demo is still at 10 AM, not 11 AM.","Friday's client demo is still at 10, not 11."],
 ['周二的预约是两点，不是四点。','The appointment on Tuesday is at 2 PM, not 4 PM.','The appointment on Tuesday is at 2, not 4.'],
 ['周五上午十点，不是十一点。','Friday at 10 a.m., not 11 a.m.','Friday at 10 a.m., not 11.'],
 ['支付页有一个按钮错位。','The payment page has one misaligned button.','The payment page has one misaligned button.'],
 ['支付页有一个按钮错位。','The payment page has a misaligned button.','The payment page has a misaligned button.'],
 ['The page has one button.','页面有一个按钮。','页面有一个按钮。'],
 ['有三个参会者。','There are three participants.','There are three participants.'],
 ['三位工程师参加。','Three engineers attend.','Three engineers attend.'],
 ['会议控制在四十五分钟。','Keep the meeting to 45 minutes.','Keep the meeting to 45 minutes.'],
 ['请给我三点建议。','Give me three suggestions.','Give me three suggestions.'],
] as [string,string,string][])assert.doesNotThrow(()=>assert.equal(translate(input,returned).outputText,expected,input),input);
for(const [input,returned] of [
 ['周五十点，不是十一点。','Friday at 11 AM, not 10 AM.'],
 ['有三个按钮。','There are two buttons.'],
 ['有三个按钮。','There are several buttons.'],
 ['有一个按钮。','There are a few buttons.'],
 ['三位工程师参加。','Four engineers attend.'],
 ['会议控制在四十五分钟。','Keep the meeting to 40 minutes.'],
 ['会议控制在四十五分钟。','Keep the meeting brief.'],
 ['周五下午三点之前不要发布。','Publish before 3 PM on Friday.'],
])assert.throws(()=>translate(input,returned),BufferedSourceFactError,input);
console.log('PASS buffered source clocks, explicit dayparts, quantity operators, matching ambiguity, recovery boundaries and unchanged input');
