export type Scenario = {
  id: string;
  label: string;
  detail?: string;
  afterMount?: () => void;
};
