// Experiment Comparison -> Sweeps (`/compare/sweeps`): the workspace's recent sweeps, each opening
// its result matrix. New sweeps are defined in the Simulation Workbench (`/sim/sweep`).

import { Link } from "react-router-dom";

import { ToolbarDivider } from "../../components/Toolbar";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { CompareTabs } from "../experiments/CompareTabs";
import { RecentSweeps } from "./SweepList";
import "./grid.css";

export default function SweepsPage() {
  return (
    <Workspace
      toolbar={
        <>
          <CompareTabs />
          <ToolbarDivider />
          <Link to={paths.sweepForm()}>New sweep</Link>
        </>
      }
      scroll
    >
      <div className="pad-3">
        <RecentSweeps />
      </div>
    </Workspace>
  );
}
