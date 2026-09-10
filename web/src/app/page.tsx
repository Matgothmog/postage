import { Account, ClaimHero } from "./Account";
import { Landing } from "./Landing";

export default function Home() {
  return <Account landing={<Landing claim={<ClaimHero />} />} />;
}
