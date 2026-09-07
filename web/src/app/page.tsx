import { Account } from "./Account";
import { Landing } from "./Landing";
import { SignInButton } from "./SignInButton";

export default function Home() {
  return <Account landing={<Landing cta={<SignInButton />} />} />;
}
