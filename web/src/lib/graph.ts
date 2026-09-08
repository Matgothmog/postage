import { required } from "./env";

/// Subgraphs published on the decentralized network, queried through the
/// gateway. These give a sender's history outside Postage.
export const ENS_SUBGRAPH = "5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH";

interface GraphResponse<T> {
  data?: T;
  errors?: { message: string }[];
}

async function query<T>(url: string, document: string, variables: object = {}): Promise<T> {
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query: document, variables }),
    cache: "no-store",
  });

  if (!response.ok) throw new Error(`Graph query failed: ${response.status}`);

  const payload = (await response.json()) as GraphResponse<T>;
  if (payload.errors?.length) throw new Error(payload.errors[0].message);
  if (!payload.data) throw new Error("Graph returned no data");
  return payload.data;
}

/// Our own Subgraph: what this sender has done inside Postage.
export function queryPostage<T>(document: string, variables?: object): Promise<T> {
  return query<T>(required("GRAPH_QUERY_URL"), document, variables);
}

/// Public Subgraphs on the network, reached through the gateway.
export function queryNetwork<T>(subgraphId: string, document: string, variables?: object): Promise<T> {
  const gateway = `https://gateway.thegraph.com/api/${required("GRAPH_API_KEY")}/subgraphs/id/${subgraphId}`;
  return query<T>(gateway, document, variables);
}
