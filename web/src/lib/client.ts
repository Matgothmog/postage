import { createPublicClient, http } from "viem";
import { chain } from "./contracts";

export const publicClient = createPublicClient({ chain, transport: http() });
