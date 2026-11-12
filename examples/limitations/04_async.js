// Test: async/await and Promises
async function fetchData() {
  return "data";
}
async function main() {
  let result = await fetchData();
  console.log(result);
}
main();

