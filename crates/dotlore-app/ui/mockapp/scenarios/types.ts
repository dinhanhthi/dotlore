export type Scenario = {
  id: string;
  label: string;
  afterMount?: () => void;
};
