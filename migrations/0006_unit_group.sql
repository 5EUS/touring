-- Add group to units
ALTER TABLE chapters ADD COLUMN "group" TEXT;
ALTER TABLE episodes ADD COLUMN "group" TEXT;