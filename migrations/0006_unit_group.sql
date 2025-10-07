-- Add group to units
ALTER TABLE chapters ADD COLUMN "upload_group" TEXT;
ALTER TABLE episodes ADD COLUMN "upload_group" TEXT;